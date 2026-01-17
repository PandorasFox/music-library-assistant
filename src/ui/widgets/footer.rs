//! Footer Widget
//!
//! Renders the application footer with corpus status, operation progress,
//! controls hints, and eye animation.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::collections::VecDeque;
use std::time::Instant;

use crate::ui::app::{EyeAnimation, EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use crate::ui::helpers::{calculate_rolling_throughput, format_bytes_binary, format_eta, truncate_path_display};

use super::ControlsHint;

/// Operation information for display in the footer
#[derive(Clone)]
pub struct OperationDisplay {
    pub description: String,
    pub completed_items: usize,
    pub total_items: usize,
    pub skipped_items: usize,
    pub bytes_processed: Option<u64>,
    pub total_bytes: Option<u64>,
    pub current_item: Option<String>,
    /// Mtime statistics for scan operations
    pub mtime_stats: Option<MtimeStats>,
}

/// Mtime statistics for scan progress display
#[derive(Clone, Default)]
pub struct MtimeStats {
    pub mismatch_count: usize,
    pub not_in_db_count: usize,
    pub mean_diff_secs: Option<f64>,
    pub median_diff_secs: Option<i64>,
    pub mode_diff_secs: Option<i64>,
    pub stddev_diff_secs: Option<f64>,
}

/// Context for rendering the footer
pub struct FooterContext<'a> {
    pub operations: Vec<OperationDisplay>,
    pub throughput_samples: &'a VecDeque<(Instant, u64)>,
    pub eye: &'a EyeAnimation,
    pub controls_hint: ControlsHint,
    pub status_message: Option<&'a str>,
}

/// Render the complete footer area
pub fn render_footer(f: &mut Frame, area: Rect, ctx: &FooterContext) {
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(30),      // Left status area (flexible)
            Constraint::Length(70),   // Eye animation (fixed 70 cols)
        ])
        .split(area);

    // Left side: three stacked status boxes
    let status_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // Corpus status
            Constraint::Length(6),  // Operation status
            Constraint::Min(4),     // Controls
        ])
        .split(footer_layout[0]);

    render_corpus_status(f, status_layout[0], ctx);
    render_operation_status(f, status_layout[1], ctx);
    render_controls(f, status_layout[2], ctx);

    // Eye animation
    render_eye(f, footer_layout[1], ctx.eye);
}

/// Render corpus health status panel
fn render_corpus_status(f: &mut Frame, area: Rect, _ctx: &FooterContext) {
    // TODO: Corpus status should show health_issues summary from eyeballing
    let lines = vec![
        Line::from("No observation data")
            .style(Style::default().fg(Color::DarkGray)),
    ];

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Corpus"));
    f.render_widget(para, area);
}

/// Render operation progress panel
fn render_operation_status(f: &mut Frame, area: Rect, ctx: &FooterContext) {
    let mut lines = Vec::new();

    if ctx.operations.is_empty() {
        lines.push(
            Line::from("No operation in progress").style(Style::default().fg(Color::DarkGray)),
        );
    } else {
        for (i, op) in ctx.operations.iter().enumerate() {
            // Operation description with ETA
            let eta_span = {
                let bytes_processed = op.bytes_processed.unwrap_or(0);
                let total_bytes = op.total_bytes.unwrap_or(0);
                if bytes_processed > 0 && total_bytes > 0 {
                    if i == 0 {
                        let throughput = calculate_rolling_throughput(ctx.throughput_samples, 8);
                        if let Some(mib_per_sec) = throughput {
                            if mib_per_sec > 0.01 {
                                let remaining_bytes = total_bytes.saturating_sub(bytes_processed);
                                let remaining_mib = remaining_bytes as f64 / (1024.0 * 1024.0);
                                let eta_secs = (remaining_mib / mib_per_sec) as u64;
                                Span::styled(
                                    format!(" (ETA: {})", format_eta(eta_secs)),
                                    Style::default().fg(Color::DarkGray),
                                )
                            } else {
                                Span::raw("")
                            }
                        } else {
                            Span::raw("")
                        }
                    } else {
                        Span::raw("")
                    }
                } else {
                    Span::raw("")
                }
            };

            lines.push(Line::from(vec![
                Span::styled(&op.description, Style::default().fg(Color::Cyan)),
                eta_span,
            ]));

            // Items progress
            if ctx.operations.len() > 1 {
                lines.push(Line::from(format!(
                    "  {}/{} ({} skipped)",
                    op.completed_items,
                    op.total_items,
                    op.skipped_items
                )));
            } else {
                lines.push(Line::from(format!(
                    "Items: {}/{} | Skipped: {} unchanged",
                    op.completed_items,
                    op.total_items,
                    op.skipped_items
                )));

                // Mtime mismatch statistics
                if let Some(ref mtime_stats) = op.mtime_stats {
                    if mtime_stats.mismatch_count > 0 || mtime_stats.not_in_db_count > 0 {
                        let mut parts = Vec::new();
                        if mtime_stats.not_in_db_count > 0 {
                            parts.push(format!("new:{}", mtime_stats.not_in_db_count));
                        }
                        if mtime_stats.mismatch_count > 0 {
                            parts.push(format!("mtime_delta:{}", mtime_stats.mismatch_count));
                            if let Some(mean) = mtime_stats.mean_diff_secs {
                                let median = mtime_stats.median_diff_secs.unwrap_or(0);
                                let mode = mtime_stats.mode_diff_secs.unwrap_or(0);
                                let stddev = mtime_stats.stddev_diff_secs.unwrap_or(0.0);
                                parts.push(format!(
                                    "μ={:.1}s med={}s mode={}s σ={:.1}",
                                    mean, median, mode, stddev
                                ));
                            }
                        }
                        lines.push(
                            Line::from(parts.join(" | ")).style(Style::default().fg(Color::DarkGray)),
                        );
                    }
                }

                // Bytes progress
                if let (Some(processed), Some(total)) = (op.bytes_processed, op.total_bytes) {
                    let processed_str = format_bytes_binary(processed);
                    let total_str = format_bytes_binary(total);
                    let throughput_str = match calculate_rolling_throughput(ctx.throughput_samples, 8) {
                        Some(mib_per_sec) => format!(" ({:.1} MiB/s)", mib_per_sec),
                        None => String::new(),
                    };
                    lines.push(Line::from(format!(
                        "Bytes: {} / {}{}",
                        processed_str, total_str, throughput_str
                    )));
                }

                // Current item
                if let Some(ref item) = op.current_item {
                    let display = truncate_path_display(item, 50);
                    lines.push(Line::from(display).style(Style::default().fg(Color::DarkGray)));
                }
            }
        }
    }

    let title = if ctx.operations.len() > 1 {
        format!("Operations ({})", ctx.operations.len())
    } else {
        "Operation".to_string()
    };

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(para, area);
}

/// Render controls hint panel
fn render_controls(f: &mut Frame, area: Rect, ctx: &FooterContext) {
    let mut lines = Vec::new();

    // Show any status message first
    if let Some(msg) = ctx.status_message {
        lines.push(Line::from(msg.to_string()).style(Style::default().fg(Color::Yellow)));
    }

    // Render controls hint
    lines.push(ctx.controls_hint.render_line());

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}

/// Render the eye animation
fn render_eye(f: &mut Frame, area: Rect, eye: &EyeAnimation) {
    let eye_text = match eye.current_frame() {
        EyeFrame::Open => EYE_OPEN,
        EyeFrame::Closing => EYE_CLOSING,
        EyeFrame::Closed => EYE_CLOSED,
    };

    // Center the 64-col eye in the 70-col frame
    let centered_eye: String = eye_text
        .lines()
        .map(|line| format!("   {}   ", line))
        .collect::<Vec<_>>()
        .join("\n");

    let eye_para = Paragraph::new(centered_eye)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(eye_para, area);
}
