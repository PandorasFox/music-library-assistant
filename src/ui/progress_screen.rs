//! Unified Progress Screen
//!
//! Full-screen progress modal for all Witch work phases:
//! - Eyeballing (startup corpus scan, eye closed)
//! - ContentAnalysis (metadata analysis, eye awake)
//! - SignalRefresh (post-mutation signal updates, eye awake)
//!
//! ## Usage
//!
//! ```rust,ignore
//! // For startup eyeballing
//! let mut progress = ProgressScreen::new_eyeballing();
//! witch.start_observing(...);
//!
//! // Each frame
//! if progress.tick(witch) {
//!     // Eyeballing complete, transition to next phase
//! }
//!
//! // For content analysis
//! let mut progress = ProgressScreen::new_content_analysis();
//! witch.queue_content_analysis();
//! ```

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::eye::{EYE_CLOSED, EYE_CLOSING};
use super::wait_state::WaitState;
use crate::db::write_thread::DbThreadStats;
use crate::witch::{ReasoningLevel, Witch, WorkStateSnapshot, WorkStatus, WorkerStats};
use std::collections::HashMap;

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
    /// Get the status message for this phase (without trailing ellipsis).
    pub fn status_message(&self, reasoning: ReasoningLevel, complete: bool) -> &'static str {
        if complete {
            return match self {
                ProgressPhase::Eyeballing => "Ready!",
                ProgressPhase::ContentAnalysis => "Analysis complete!",
                ProgressPhase::SignalRefresh => "Signals updated!",
            };
        }

        match self {
            ProgressPhase::Eyeballing => match reasoning {
                ReasoningLevel::None => "Scanning corpus",
                ReasoningLevel::Inodes => "Computing health signals",
                ReasoningLevel::Full => "Ready!",
            },
            ProgressPhase::ContentAnalysis => "Analyzing metadata",
            ProgressPhase::SignalRefresh => "Updating signals",
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
/// Handles all Witch work phases with a consistent interface.
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
    /// Current reasoning level (for Eyeballing phase rendering).
    reasoning_level: ReasoningLevel,
    /// DB thread stats for optional display.
    db_stats: Option<DbThreadStats>,
    /// Worker thread stats for optional display.
    worker_stats: Option<WorkerStats>,
    /// Pending DB writes (always tracked).
    db_queue_depth: u64,
    /// Consecutive ticks where the Witch was idle (safety valve for missed work).
    consecutive_idle_ticks: u8,
    /// Tick counter for animations (spinner, color pulse).
    tick_count: u32,
    /// Last time the animation ticked (for frame-rate independent animation).
    last_animation_tick: Instant,
    /// Breakdown of pending tasks by type label (e.g., "Indexing": 42).
    pending_counts: HashMap<String, usize>,
}

impl ProgressScreen {
    /// Create a progress screen for startup eyeballing.
    ///
    /// Completes when witch.reasoning_level() becomes Full.
    pub fn new_eyeballing() -> Self {
        Self {
            phase: ProgressPhase::Eyeballing,
            wait_state: WaitState::new(),
            progress: Some(0.0),
            progress_detail: Some("Starting...".to_string()),
            complete: false,
            reasoning_level: ReasoningLevel::None,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
            consecutive_idle_ticks: 0,
            tick_count: 0,
            last_animation_tick: Instant::now(),
            pending_counts: HashMap::new(),
        }
    }

    /// Create a progress screen for content analysis.
    ///
    /// Completes when Witch work drains after starting.
    pub fn new_content_analysis() -> Self {
        let mut screen = Self {
            phase: ProgressPhase::ContentAnalysis,
            wait_state: WaitState::new(),
            progress: Some(0.0),
            progress_detail: Some("Starting metadata analysis...".to_string()),
            complete: false,
            reasoning_level: ReasoningLevel::Full,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
            consecutive_idle_ticks: 0,
            tick_count: 0,
            last_animation_tick: Instant::now(),
            pending_counts: HashMap::new(),
        };
        screen.wait_state.start();
        screen
    }

    /// Create a progress screen for post-mutation signal refresh.
    ///
    /// Completes when Witch work drains after starting.
    pub fn new_signal_refresh() -> Self {
        let mut screen = Self {
            phase: ProgressPhase::SignalRefresh,
            wait_state: WaitState::new(),
            progress: Some(0.0),
            progress_detail: Some("Updating signals...".to_string()),
            complete: false,
            reasoning_level: ReasoningLevel::Full,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
            consecutive_idle_ticks: 0,
            tick_count: 0,
            last_animation_tick: Instant::now(),
            pending_counts: HashMap::new(),
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

    /// Get the status message.
    pub fn status_message(&self) -> &'static str {
        self.phase
            .status_message(self.reasoning_level, self.complete)
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

    /// Get the current bouncing dot spinner character for left column only.
    /// Used for even half-positions (left column bouncing, right empty).
    pub fn spinner_char_left(&self) -> char {
        // Single dot bouncing up/down in left column (dots 1,2,3)
        const BOUNCE_LEFT: &[char] = &['⠁', '⠂', '⠄', '⠂']; // top, mid, bottom, mid
        BOUNCE_LEFT[((self.tick_count / 2) as usize) % BOUNCE_LEFT.len()]
    }

    /// Get the current bouncing dot spinner character for right column with left filled.
    /// Used for odd half-positions (left column filled + right column bouncing).
    pub fn spinner_char_right(&self) -> char {
        // Left column filled + bouncing dot in right column
        const BOUNCE_RIGHT_WITH_LEFT: &[char] = &['⠏', '⠗', '⠧', '⠗']; // 1,2,3+4 / +5 / +6 / +5
        BOUNCE_RIGHT_WITH_LEFT[((self.tick_count / 2) as usize) % BOUNCE_RIGHT_WITH_LEFT.len()]
    }

    /// Animation tick interval in milliseconds (~10Hz for smooth animation).
    const ANIMATION_TICK_MS: u64 = 100;

    /// Tick the progress screen state.
    ///
    /// Returns `true` if work is complete.
    pub fn tick(&mut self, witch: &Witch) -> bool {
        // Time-based animation tick: only increment when enough time has elapsed.
        // This keeps animation smooth regardless of UI frame rate.
        let now = Instant::now();
        let elapsed_ms = now.duration_since(self.last_animation_tick).as_millis() as u64;
        if elapsed_ms >= Self::ANIMATION_TICK_MS {
            // Advance by the number of ticks that should have occurred
            let ticks_to_add = elapsed_ms / Self::ANIMATION_TICK_MS;
            self.tick_count = self.tick_count.wrapping_add(ticks_to_add as u32);
            self.last_animation_tick = now;
        }

        // Update progress from the Witch
        let status = witch.status();
        self.update_progress(&status);

        // Update stats
        self.set_db_stats(witch.db_stats());
        self.set_worker_stats(witch.worker_stats());
        self.set_db_queue_depth(witch.db_queue_depth());

        // Phase-specific completion detection
        match self.phase {
            ProgressPhase::Eyeballing => {
                // Track reasoning level for rendering
                self.reasoning_level = witch.reasoning_level();

                // Complete when reasoning reaches Full
                if self.reasoning_level == ReasoningLevel::Full {
                    self.complete = true;
                }
            }
            ProgressPhase::ContentAnalysis | ProgressPhase::SignalRefresh => {
                // Never complete until DB writes are flushed
                let writes_flushed = self.db_queue_depth == 0;
                if !writes_flushed {
                    self.consecutive_idle_ticks = 0;
                    return false;
                }

                // Use WaitState for completion detection
                if self.wait_state.tick(witch) {
                    self.complete = true;
                } else {
                    // Safety valve: if the Witch has been idle for 3 consecutive ticks,
                    // assume work already completed before we started watching
                    let is_idle = status.pending == 0
                        && matches!(
                            status.state,
                            WorkStateSnapshot::Idle | WorkStateSnapshot::Done
                        );
                    if is_idle {
                        self.consecutive_idle_ticks = self.consecutive_idle_ticks.saturating_add(1);
                        if self.consecutive_idle_ticks >= 3 {
                            self.complete = true;
                        }
                    } else {
                        self.consecutive_idle_ticks = 0;
                    }
                }
            }
        }

        self.complete
    }

    /// Update progress from Witch status.
    fn update_progress(&mut self, status: &WorkStatus) {
        let completed = status.total_processed;
        let total = status.session_queued;
        if total > 0 {
            self.progress = Some(completed as f32 / total as f32);
            self.progress_detail = Some(format!("{} / {}", completed, total));
        }
        // Store pending task breakdown for display (shows remaining work)
        self.pending_counts = status.pending_by_label.clone();
    }

    /// Format pending task counts as a compact summary string.
    ///
    /// Returns something like "Indexing: 42 | Tag sync: 12 | File move: 3"
    /// Sorted by count (descending), limited to fit reasonable width.
    /// Shows remaining work (decreasing counts as tasks complete).
    fn format_task_summary(&self) -> Option<String> {
        if self.pending_counts.is_empty() {
            return None;
        }

        // Sort by count descending, then alphabetically for ties
        let mut entries: Vec<_> = self.pending_counts.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

        // Build summary, limiting to 64 chars for display (matches eye art width)
        let mut parts = Vec::new();
        let mut total_len = 0;
        const MAX_LEN: usize = 64;

        for (label, count) in entries {
            let part = format!("{}: {}", label, count);
            let part_len = part.len() + 3; // " | " separator

            if total_len + part_len > MAX_LEN && !parts.is_empty() {
                // Would exceed limit, add "..." and stop
                parts.push("…".to_string());
                break;
            }

            total_len += part_len;
            parts.push(part);
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" │ "))
        }
    }

    /// Get the eye art for this phase.
    pub fn eye_art(&self) -> &'static str {
        match self.phase {
            ProgressPhase::Eyeballing => match self.reasoning_level {
                ReasoningLevel::Inodes => EYE_CLOSING, // Half-open frame
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
// Animated Color
// ============================================================================

/// Calculate animated color with sinusoidal hue/saturation/lightness shifting.
///
/// Base color is #f15c99 (RGB 241, 92, 153) - a vibrant magenta/pink.
/// Hue shifts ±7% with period ~2.2 seconds (22 ticks at 100ms).
/// Saturation shifts ±3.6% with period ~3.3 seconds (33 ticks).
/// Lightness shifts ±1.8% with period ~5.5 seconds (55 ticks).
fn animated_progress_color(tick: u32) -> Color {
    // Base color #f15c99 in HSL: H=337°, S=83%, L=65%
    let base_h = 337.0_f32;
    let base_s = 0.83_f32;
    let base_l = 0.65_f32;

    // Sinusoidal shifts with different periods for variety
    // (slowed 10% and tightened 10% from initial values)
    let tick_f = tick as f32;
    let hue_shift = (tick_f * 0.285).sin() * 24.3; // ±24° (~7% of 360), period ~22 ticks
    let sat_shift = (tick_f * 0.190).sin() * 0.036; // ±3.6%, period ~33 ticks
    let light_shift = (tick_f * 0.115).sin() * 0.018; // ±1.8%, period ~55 ticks

    let h = (base_h + hue_shift).rem_euclid(360.0);
    let s = (base_s + sat_shift).clamp(0.0, 1.0);
    let l = (base_l + light_shift).clamp(0.0, 1.0);

    // HSL to RGB conversion
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    let r = ((r1 + m) * 255.0).round() as u8;
    let g = ((g1 + m) * 255.0).round() as u8;
    let b = ((b1 + m) * 255.0).round() as u8;

    Color::Rgb(r, g, b)
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
    let eye_progress_spacing = if screen.progress.is_some() { 1 } else { 0 }; // Blank line above progress

    // Task summary (shows breakdown by type)
    let task_summary = screen.format_task_summary();
    let task_summary_height = if task_summary.is_some() { 1 } else { 0 };

    // Always reserve space for queue depth line to prevent layout jumping
    let show_queue_depth = screen.db_queue_depth > 0;
    let queue_depth_height = 1; // Always 1 to stabilize layout

    // Detailed timing stats (behind timing guard)
    let show_stats = crate::config::is_timing_enabled();
    let show_db_stats = show_stats && screen.db_stats.is_some();
    let show_worker_stats = show_stats && screen.worker_stats.is_some();
    let db_stats_height = if show_db_stats { 1 } else { 0 };
    let worker_stats_height = if show_worker_stats { 2 } else { 0 };
    let total_height = 2
        + eye_height
        + eye_progress_spacing
        + progress_height
        + task_summary_height
        + queue_depth_height
        + db_stats_height
        + worker_stats_height;

    // Calculate vertical centering
    let v_margin = area.height.saturating_sub(total_height as u16) / 2;

    // Calculate horizontal centering for the eye (eye art is 64 chars wide)
    let eye_width = 64u16;
    let h_margin = area.width.saturating_sub(eye_width) / 2;

    // Layout vertically
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(v_margin),                    // 0: Top margin
            Constraint::Length(1),                           // 1: Message
            Constraint::Length(1),                           // 2: Spacing
            Constraint::Length(eye_height as u16),           // 3: Eye
            Constraint::Length(eye_progress_spacing as u16), // 4: Blank line above progress
            Constraint::Length(progress_height as u16),      // 5: Progress bar
            Constraint::Length(task_summary_height as u16),  // 6: Task summary
            Constraint::Length(queue_depth_height as u16),   // 7: Queue depth
            Constraint::Length(db_stats_height as u16),      // 8: DB stats
            Constraint::Length(worker_stats_height as u16),  // 9: Worker stats
            Constraint::Min(0),                              // 10: Bottom margin
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

    let eye_lines: Vec<Line> = eye_art.lines().map(Line::from).collect();

    let eye_widget = Paragraph::new(eye_lines).style(Style::default().fg(Color::DarkGray));
    f.render_widget(eye_widget, eye_area);

    // Render progress bar if present
    if let Some(progress) = screen.progress {
        let progress_area = chunks[5]; // Now at index 5 due to new spacing row

        let bar_inner_width = 40u16.min(progress_area.width.saturating_sub(4));
        let bar_x = (progress_area.width.saturating_sub(bar_inner_width + 2)) / 2 + progress_area.x;

        // Calculate filled portion with half-cell granularity
        // Each braille char has left and right columns; we track progress in half-cells
        // Sequence: left spinner → right-with-left → full block + left spinner → ...
        let max_half = bar_inner_width * 2;
        let half_filled = ((progress * max_half as f32) as u16).min(max_half.saturating_sub(1));
        let full_blocks = half_filled / 2;
        let is_half_fill = half_filled % 2 == 1;
        let empty = bar_inner_width.saturating_sub(full_blocks + 1);

        // Get animated color
        let bar_color = animated_progress_color(screen.tick_count);

        // Build the bar:
        // - Brackets on inside columns: ⠸ (right col) left, ⠇ (left col) right
        // - Fill uses ⠿ (full 6-dot braille)
        // - Even positions: left-only spinner (⠁⠂⠄)
        // - Odd positions: right-with-left spinner (⠏⠗⠧) for half-fill effect
        let spinner = if is_half_fill {
            screen.spinner_char_right()
        } else {
            screen.spinner_char_left()
        };
        let bar_text = format!(
            "⠸{}{}{}⠇",
            "⠿".repeat(full_blocks as usize),
            spinner,
            " ".repeat(empty as usize)
        );

        let detail = screen.progress_detail.as_deref().unwrap_or("");
        let progress_text = format!("{}\n{}", bar_text, detail);

        let progress_widget = Paragraph::new(progress_text)
            .style(Style::default().fg(bar_color))
            .alignment(Alignment::Center);

        let centered_progress = Rect {
            x: bar_x,
            y: progress_area.y,
            width: bar_inner_width + 4,
            height: progress_area.height,
        };

        f.render_widget(progress_widget, centered_progress);
    }

    // Render task summary (breakdown by type) - centered to match eye width (64 chars)
    if let Some(summary) = task_summary {
        let summary_area = chunks[6];

        let summary_line = Line::from(Span::styled(summary, Style::default().fg(Color::DarkGray)));
        let summary_widget = Paragraph::new(summary_line).alignment(Alignment::Center);

        let centered_summary = Rect {
            x: summary_area
                .x
                .saturating_add((summary_area.width.saturating_sub(64)) / 2),
            y: summary_area.y,
            width: 64.min(summary_area.width),
            height: summary_area.height,
        };
        f.render_widget(summary_widget, centered_summary);
    }

    // Render pending DB writes
    if show_queue_depth {
        let queue_area = chunks[7]; // Index shifted due to task summary row
        let label_color = Color::Rgb(245, 28, 153); // Magenta

        let queue_line = Line::from(vec![Span::styled(
            format!("[{} pending DB writes queued]", screen.db_queue_depth),
            Style::default().fg(label_color),
        )]);

        let queue_widget = Paragraph::new(queue_line).alignment(Alignment::Center);
        f.render_widget(queue_widget, queue_area);
    }

    // Render DB stats if timing enabled
    if show_db_stats {
        if let Some(stats) = &screen.db_stats {
            let stats_area = chunks[8]; // Index shifted due to task summary row

            let queue_color = if stats.queue_depth > 100 {
                Color::Red
            } else if stats.queue_depth > 0 {
                Color::Yellow
            } else {
                Color::Green
            };

            let stats_line = Line::from(vec![
                Span::styled("Writes: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", stats.total_writes),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.1}/s", stats.writes_per_sec),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(" | Q: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", stats.queue_depth),
                    Style::default().fg(queue_color),
                ),
            ]);

            let stats_widget = Paragraph::new(stats_line).alignment(Alignment::Center);
            f.render_widget(stats_widget, stats_area);
        }
    }

    // Render worker stats if timing enabled
    if show_worker_stats {
        if let Some(stats) = &screen.worker_stats {
            let stats_area = chunks[9]; // Index shifted due to task summary row
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
                Span::styled(
                    format!("{}", stats.tasks_completed),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled("Avg: ", Style::default().fg(label_color)),
                Span::styled(
                    format!("{}ms", stats.avg_task_ms),
                    Style::default().fg(avg_task_color),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled("Max: ", Style::default().fg(label_color)),
                Span::styled(
                    format!("{}ms", stats.max_task_ms),
                    Style::default().fg(Color::White),
                ),
                if !stats.max_task_label.is_empty() {
                    Span::styled(
                        format!(" ({})", stats.max_task_label),
                        Style::default().fg(Color::DarkGray),
                    )
                } else {
                    Span::raw("")
                },
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled("Threads: ", Style::default().fg(label_color)),
                Span::styled(
                    format!("{}", stats.active_threads),
                    Style::default().fg(Color::White),
                ),
            ]);

            let line2 = Line::from(vec![
                Span::styled("Q Wait: ", Style::default().fg(label_color)),
                Span::styled(
                    format!("{}avg", stats.queue_wait_avg_ms),
                    Style::default().fg(queue_wait_color),
                ),
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}max ms", stats.queue_wait_max_ms),
                    Style::default().fg(Color::White),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled("Reads: ", Style::default().fg(label_color)),
                Span::styled(
                    format!("{}", stats.avg_db_read_us),
                    Style::default().fg(Color::Green),
                ),
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", stats.median_db_read_us),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}µs", stats.max_db_read_us),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!(" ({})", stats.total_db_reads),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let worker_stats_widget =
                Paragraph::new(vec![line1, line2]).alignment(Alignment::Center);
            f.render_widget(worker_stats_widget, stats_area);
        }
    }
}
