//! Main rendering functions for the TUI.
//!
//! This module contains all top-level render functions that dispatch
//! to mode-specific renderers.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::collections::VecDeque;
use std::time::Instant;

use crate::config::Config;

use super::app::{EyeAnimation, EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use super::helpers::format_duration;
use super::widgets::{control_presets, Modal, ModalButton, ModalStyle};
use super::{deploy_flow, insights_view, tag_editor, tag_search, tree_browser};

/// Display context passed to rendering functions.
/// Contains all the state needed to render the UI.
pub struct RenderContext<'a> {
    pub mode: super::UiMode,
    pub config: &'a Config,
    pub status_message: Option<&'a str>,
    pub tree_browser: Option<&'a mut tree_browser::TreeBrowserState>,
    pub deployment_preview: Option<&'a mut deploy_flow::DeploymentPreviewState>,
    pub unified_tag_editor: Option<&'a mut tag_editor::UnifiedTagEditorState>,
    pub exit_confirm_modal_state: Option<&'a super::ExitConfirmModalState>,
    pub splash_screen: Option<&'a super::splash_screen::SplashScreen>,
    pub content_analysis: Option<&'a super::startup::ContentAnalysisProgress>,
    pub insights_view: Option<&'a mut insights_view::InsightsViewState>,
    pub tag_search: Option<&'a tag_search::TagSearchState>,
    pub intake_confirmation: Option<&'a super::startup::IntakeConfirmationState>,
    pub eye: &'a EyeAnimation,
    pub throughput_samples: &'a VecDeque<(Instant, u64)>,
    pub daemon_status: Option<crate::daemon::DaemonStatus>,
    pub corpus_summary: Option<crate::corpus::db::types::CorpusSummary>,
    pub db_stats: Option<crate::db_thread::DbThreadStats>,
}

/// Main render entry point - dispatches to sub-renderers based on mode.
pub fn render(f: &mut Frame, ctx: &mut RenderContext) {
    // Loading splash takes the whole screen
    if ctx.mode == super::UiMode::LoadingSplash {
        if let Some(splash) = ctx.splash_screen {
            super::splash_screen::render(f, f.area(), splash, ctx.config);
        }
        return;
    }

    // Content analysis progress takes the whole screen (with awake eye)
    if ctx.mode == super::UiMode::ContentAnalysis {
        if let Some(progress) = ctx.content_analysis {
            // Get current eye frame for animated display
            let eye_frame = match ctx.eye.current_frame() {
                EyeFrame::Open => EYE_OPEN,
                EyeFrame::Closing => EYE_CLOSING,
                EyeFrame::Closed => EYE_CLOSED,
            };
            super::startup::content_analysis::render(f, f.area(), progress, eye_frame);
        }
        return;
    }

    // Lateral views (Tag Search, Corpus Browser, Insights, Deploy) have their own title bar
    // and get the full header+content area
    let uses_unified_titlebar = matches!(
        ctx.mode,
        super::UiMode::TagSearch
            | super::UiMode::CorpusBrowser
            | super::UiMode::Insights
            | super::UiMode::DeploymentPreview
    );

    if uses_unified_titlebar {
        // Two-part layout: content (with unified titlebar) + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),    // Content with unified titlebar
                Constraint::Length(18), // Footer with eye
            ])
            .split(f.area());

        render_content(f, chunks[0], ctx);
        render_footer(f, chunks[1], ctx);
    } else {
        // Standard three-part layout: header + content + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(10),    // Content
                Constraint::Length(18), // Footer with eye
            ])
            .split(f.area());

        render_header(f, chunks[0], ctx);
        render_content(f, chunks[1], ctx);
        render_footer(f, chunks[2], ctx);
    }
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Get mode-specific suffix (if any)
    let suffix = match ctx.mode {
        super::UiMode::ContentAnalysis => None, // Never reached - handled separately
        super::UiMode::DirBrowser => Some("Directory Browser"),
        super::UiMode::DeploymentPreview => Some("Deployment Preview"),
        super::UiMode::ExitConfirmModal => Some("Exit Confirmation"),
        super::UiMode::CorpusBrowser => Some("Corpus Browser"),
        super::UiMode::Insights => Some("Corpus Insights"),
        super::UiMode::LoadingSplash => None, // Never reached - handled separately
        super::UiMode::IntakeConfirmation => Some("Intake Confirmation"),
        super::UiMode::TagSearch => Some("Tag Search"),
        super::UiMode::UnifiedTagEditor => Some("Tag Editor"),
    };

    let title = match suffix {
        Some(s) => format!("{} - {}", crate::MLA_TITLE, s),
        None => crate::MLA_TITLE.to_string(),
    };

    let header = Paragraph::new(title)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn render_content(f: &mut Frame, area: ratatui::layout::Rect, ctx: &mut RenderContext) {
    match ctx.mode {
        super::UiMode::ContentAnalysis => {
            // Never reached - handled separately in render() before this function
        }
        super::UiMode::DirBrowser => {
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::DeploymentPreview => {
            if let Some(ref mut preview) = ctx.deployment_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::ExitConfirmModal => {
            render_exit_confirm_modal(f, area, ctx.exit_confirm_modal_state);
        }
        super::UiMode::CorpusBrowser => {
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::Insights => {
            if let Some(ref mut view) = ctx.insights_view {
                insights_view::render_insights_view(f, area, view);
            } else {
                // Show loading state while insights view is initializing
                render_insights_loading(f, area);
            }
        }
        super::UiMode::TagSearch => {
            if let Some(ref state) = ctx.tag_search {
                state.render(f, area);
            }
        }
        super::UiMode::LoadingSplash => {
            // Never reached - handled separately in render() before this function
        }
        super::UiMode::IntakeConfirmation => {
            if let Some(ref state) = ctx.intake_confirmation {
                super::startup::intake_confirmation::render(f, area, state);
            }
        }
        super::UiMode::UnifiedTagEditor => {
            if let Some(ref mut editor) = ctx.unified_tag_editor {
                editor.render(f, area, ctx.status_message);
            }
        }
    }
}

// render_loading_splash has been moved to splash_screen::render()

/// Render loading state for Insights view while initializing
fn render_insights_loading(f: &mut Frame, area: ratatui::layout::Rect) {
    use super::widgets::{LateralView, UnifiedTitleBar};

    // Layout: unified titlebar + content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()),
            Constraint::Min(0),
        ])
        .split(area);

    // Render unified titlebar
    let titlebar = UnifiedTitleBar::new(LateralView::Insights);
    titlebar.render(f, chunks[0]);

    // Render loading message
    let loading = Paragraph::new("Initializing insights view...")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Insights"));

    f.render_widget(loading, chunks[1]);
}

fn render_exit_confirm_modal(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    state: Option<&super::ExitConfirmModalState>,
) {
    let selected_no = state.map(|s| s.selected_no).unwrap_or(true);
    let has_operations = state.map(|s| s.has_operations).unwrap_or(false);

    // Build buttons with appropriate styles
    let (yes_btn, no_btn) = if has_operations {
        // Warning modal: Yes is red (dangerous), No is green (safe)
        let yes_btn = ModalButton::new("Yes", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Red),
                Style::default().fg(Color::White),
            );
        let no_btn = ModalButton::new("No", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Green),
                Style::default().fg(Color::White),
            );
        if selected_no {
            (yes_btn, no_btn.selected())
        } else {
            (yes_btn.selected(), no_btn)
        }
    } else {
        // Simple exit: neutral styles
        let confirm_btn = ModalButton::new("Confirm", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Cyan),
                Style::default().fg(Color::White),
            );
        let cancel_btn = ModalButton::new("Cancel", "")
            .with_indicator()
            .styles(
                Style::default().fg(Color::Black).bg(Color::Gray),
                Style::default().fg(Color::White),
            );
        if selected_no {
            (confirm_btn, cancel_btn.selected())
        } else {
            (confirm_btn.selected(), cancel_btn)
        }
    };

    // Build button line
    let mut button_spans = yes_btn.render_with_indicator();
    button_spans.push(Span::raw("     "));
    button_spans.extend(no_btn.render_with_indicator());
    let button_line = Line::from(button_spans);

    if has_operations {
        // Warning modal for operations in progress
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Operation In Progress",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("An operation is currently running."),
            Line::from("Exiting now may leave your library"),
            Line::from("in an inconsistent state."),
            Line::from(""),
            Line::from(Span::styled(
                "No guarantees about safety or integrity.",
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(""),
            Line::from("Exit anyway?"),
            Line::from(""),
            button_line,
        ];

        Modal::new()
            .title(" Warning ")
            .content(content)
            .size(50, 35)
            .style(ModalStyle::warning())
            .centered()
            .render(f, area);
    } else {
        // Simple exit confirmation
        let content = vec![
            Line::from(""),
            Line::from("Exit MLA?"),
            Line::from(""),
            Line::from(""),
            button_line,
        ];

        Modal::new()
            .title(" Exit ")
            .content(content)
            .size(35, 20)
            .style(ModalStyle::info())
            .centered()
            .render(f, area);
    }
}

fn render_footer(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
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
    render_eye(f, footer_layout[1], ctx);
}

fn render_corpus_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Split horizontally: corpus stats | db thread stats
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: Corpus stats
    let corpus_lines = if let Some(ref summary) = ctx.corpus_summary {
        let mut lines = vec![
            Line::from(vec![
                Span::raw("Tracks: "),
                Span::styled(
                    summary.track_count.to_string(),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
        ];

        // Deployment percentage if available
        if let Some(ref deploy) = summary.deployment_stats {
            lines.push(Line::from(vec![
                Span::raw("Deploy: "),
                Span::styled(
                    format!("{:.0}%", deploy.deployment_percentage),
                    Style::default().fg(Color::Green),
                ),
            ]));
        }

        // Health issues summary
        let total_issues = summary.health_summary.total_issues;
        if total_issues > 0 || summary.deploy_conflicts > 0 {
            let issue_color = if total_issues > 10 { Color::Red } else { Color::Yellow };
            lines.push(Line::from(vec![
                Span::raw("Issues: "),
                Span::styled(
                    format!("{}", total_issues),
                    Style::default().fg(issue_color),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("Healthy", Style::default().fg(Color::Green)),
            ]));
        }

        lines
    } else {
        vec![Line::from("No data").style(Style::default().fg(Color::DarkGray))]
    };

    let corpus_para = Paragraph::new(corpus_lines)
        .block(Block::default().borders(Borders::ALL).title("Corpus"));
    f.render_widget(corpus_para, split[0]);

    // Right: DB thread stats
    let db_lines = if let Some(ref stats) = ctx.db_stats {
        let rate_str = if stats.writes_per_sec >= 1.0 {
            format!("{:.0}/s", stats.writes_per_sec)
        } else if stats.writes_per_sec > 0.0 {
            format!("{:.1}/s", stats.writes_per_sec)
        } else {
            "0/s".to_string()
        };

        let latency_str = if stats.avg_latency_us > 1000 {
            format!("{}ms", stats.avg_latency_us / 1000)
        } else {
            format!("{}µs", stats.avg_latency_us)
        };

        vec![
            Line::from(vec![
                Span::raw("Writes: "),
                Span::styled(
                    stats.total_writes.to_string(),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Line::from(vec![
                Span::raw("Rate: "),
                Span::styled(rate_str, Style::default().fg(Color::Green)),
                Span::raw(" • "),
                Span::styled(latency_str, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::raw("Queue: "),
                Span::styled(
                    stats.queue_depth.to_string(),
                    if stats.queue_depth > 100 {
                        Style::default().fg(Color::Red)
                    } else if stats.queue_depth > 0 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::Green)
                    },
                ),
            ]),
        ]
    } else {
        vec![Line::from("No DB thread").style(Style::default().fg(Color::DarkGray))]
    };

    let db_para = Paragraph::new(db_lines)
        .block(Block::default().borders(Borders::ALL).title("DB Thread"));
    f.render_widget(db_para, split[1]);
}

fn render_operation_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    let has_daemon_work = ctx.daemon_status.as_ref().map(|s| s.pending > 0).unwrap_or(false);
    let has_completed_session = ctx.daemon_status.as_ref()
        .and_then(|s| s.completed_session.as_ref())
        .is_some();

    if has_daemon_work {
        // Show active daemon status
        if let Some(ref status) = ctx.daemon_status {
            // Build task type summary
            let task_summary: String = if !status.task_counts.is_empty() {
                status.task_counts.iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                "Processing...".to_string()
            };

            lines.push(Line::from(vec![
                Span::styled("Task Daemon ", Style::default().fg(Color::Cyan)),
                Span::styled(task_summary, Style::default().fg(Color::White)),
            ]));

            // Show elapsed time
            let elapsed_str = status.elapsed.map(|d| format_duration(d)).unwrap_or_default();
            lines.push(Line::from(format!(
                "Pending: {} | Processed: {}{}",
                status.pending,
                status.total_processed,
                if !elapsed_str.is_empty() { format!(" | {}", elapsed_str) } else { String::new() }
            )));

            if !status.recent_errors.is_empty() {
                lines.push(Line::from(
                    status.recent_errors.last().unwrap_or(&String::new()).clone()
                ).style(Style::default().fg(Color::Red)));
            }
        }
    } else if has_completed_session {
        // Show lingering completed session summary
        if let Some(ref status) = ctx.daemon_status {
            if let Some(ref session) = status.completed_session {
                // Build task type breakdown
                let task_breakdown: String = session.task_counts.iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ");

                let status_color = if session.failed > 0 { Color::Yellow } else { Color::Green };
                lines.push(Line::from(vec![
                    Span::styled("Completed ", Style::default().fg(status_color)),
                    Span::styled(task_breakdown, Style::default().fg(Color::White)),
                ]));

                let duration_str = format_duration(session.duration);
                let failed_str = if session.failed > 0 {
                    format!(" | {} failed", session.failed)
                } else {
                    String::new()
                };
                lines.push(Line::from(format!(
                    "{} tasks in {}{}",
                    session.total_processed,
                    duration_str,
                    failed_str
                )).style(Style::default().fg(Color::DarkGray)));
            }
        }
    } else {
        // No active work
        lines.push(
            Line::from("No operation in progress").style(Style::default().fg(Color::DarkGray)),
        );
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Task"));
    f.render_widget(para, area);
}

fn render_controls(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    // Show any status message first
    if let Some(msg) = ctx.status_message {
        lines.push(Line::from(msg.to_string()).style(Style::default().fg(Color::Yellow)));
    }

    // Get mode-specific controls hint
    let controls = match ctx.mode {
        super::UiMode::ContentAnalysis => control_presets::empty(), // No controls during analysis
        super::UiMode::DirBrowser => control_presets::dir_browser(),
        super::UiMode::DeploymentPreview => control_presets::deployment_preview(),
        super::UiMode::ExitConfirmModal => control_presets::exit_confirm_modal(),
        super::UiMode::CorpusBrowser => control_presets::corpus_browser(),
        super::UiMode::Insights => control_presets::insights_view(),
        super::UiMode::TagSearch => control_presets::tag_search(),
        super::UiMode::LoadingSplash => control_presets::empty(),
        super::UiMode::IntakeConfirmation => control_presets::empty(), // Modal handles its own hints
        super::UiMode::UnifiedTagEditor => control_presets::tag_editor(), // Reuse same controls
    };
    lines.push(controls.render_line());

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}

fn render_eye(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let eye_text = match ctx.eye.current_frame() {
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
