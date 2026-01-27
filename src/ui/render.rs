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

use crate::config::{self, Config};

use super::app::{EyeAnimation, EyeFrame, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use super::helpers::format_duration;
use super::widgets::{control_presets, Modal, ModalButton, ModalStyle};
use super::{compound_split, deploy_flow, format_standardization, insights_view, missing_file_flow, tag_canonicity, tag_editor, tag_search, transaction_review, tree_browser};

/// Display context passed to rendering functions.
/// Contains all the state needed to render the UI.
pub struct RenderContext<'a> {
    pub mode: super::UiMode,
    pub config: &'a Config,
    pub status_message: Option<&'a str>,
    pub tree_browser: Option<&'a mut tree_browser::TreeBrowserState>,
    pub deployment_preview: Option<&'a mut deploy_flow::DeploymentPreviewState>,
    pub missing_file_preview: Option<&'a missing_file_flow::MissingFilePreviewState>,
    pub tag_canonicity_state: Option<&'a tag_canonicity::TagCanonicalityState>,
    pub compound_split_state: Option<&'a compound_split::CompoundSplitState>,
    pub transaction_review: Option<&'a transaction_review::TransactionReviewState>,
    pub transaction_review_decisions: Vec<transaction_review::DecisionSummary>,
    pub unified_tag_editor: Option<&'a mut tag_editor::UnifiedTagEditorState>,
    pub exit_confirm_modal_state: Option<&'a super::ExitConfirmModalState>,
    pub progress_screen: Option<&'a super::progress_screen::ProgressScreen>,
    pub insights_view: Option<&'a mut insights_view::InsightsViewState>,
    pub tag_search: Option<&'a tag_search::TagSearchState>,
    pub format_std: Option<&'a format_standardization::FormatStdState>,
    pub intake_confirmation: Option<&'a super::startup::IntakeConfirmationState>,
    pub eye: &'a EyeAnimation,
    pub throughput_samples: &'a VecDeque<(Instant, u64)>,
    pub witch_status: Option<crate::witch::DaemonStatus>,
    pub corpus_summary: Option<crate::corpus::db::types::CorpusSummary>,
    pub db_stats: Option<crate::db_thread::DbThreadStats>,
}

/// Main render entry point - dispatches to sub-renderers based on mode.
pub fn render(f: &mut Frame, ctx: &mut RenderContext) {
    // Progress screen takes the whole screen (startup, content analysis, signal refresh)
    if ctx.mode == super::UiMode::Progress {
        if let Some(progress) = ctx.progress_screen {
            // For non-eyeballing phases, provide animated eye frame
            let eye_frame = if progress.uses_closed_eye() {
                None // Uses its own closed/awakening eye
            } else {
                // Awake phase - use animated eye
                Some(match ctx.eye.current_frame() {
                    EyeFrame::Open => EYE_OPEN,
                    EyeFrame::Closing => EYE_CLOSING,
                    EyeFrame::Closed => EYE_CLOSED,
                })
            };
            let start = Instant::now();
            super::progress_screen::render(f, f.area(), progress, eye_frame);
            let elapsed = start.elapsed();
            if elapsed.as_millis() > 16 {
                let _ = config::log_message(&format!(
                    "[RENDER DEBUG] progress_screen::render took {}ms",
                    elapsed.as_millis()
                ));
            }
        }
        return;
    }

    // Lateral views (Tag Search, Corpus Browser, Insights, Deploy) have their own title bar
    // and get the full header+content area
    // DeploymentPreview is NOT part of the lateral ring anymore - it renders its own title
    let uses_unified_titlebar = matches!(
        ctx.mode,
        super::UiMode::TagSearch
            | super::UiMode::CorpusBrowser
            | super::UiMode::Insights
            | super::UiMode::FormatStandardization
    );

    if uses_unified_titlebar {
        // Two-part layout: content (with unified titlebar) + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10), // Content with unified titlebar
                Constraint::Length(10), // Footer
            ])
            .split(f.area());

        let start = Instant::now();
        render_content(f, chunks[0], ctx);
        let content_time = start.elapsed();

        let start = Instant::now();
        render_footer(f, chunks[1], ctx);
        let footer_time = start.elapsed();

        if content_time.as_millis() > 16 || footer_time.as_millis() > 16 {
            let _ = config::log_message(&format!(
                "[RENDER DEBUG] mode={:?} content={}ms footer={}ms",
                ctx.mode,
                content_time.as_millis(),
                footer_time.as_millis()
            ));
        }
    } else {
        // Standard three-part layout: header + content + footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(10),    // Content
                Constraint::Length(10), // Footer
            ])
            .split(f.area());

        let start = Instant::now();
        render_header(f, chunks[0], ctx);
        let header_time = start.elapsed();

        let start = Instant::now();
        render_content(f, chunks[1], ctx);
        let content_time = start.elapsed();

        let start = Instant::now();
        render_footer(f, chunks[2], ctx);
        let footer_time = start.elapsed();

        if header_time.as_millis() > 16 || content_time.as_millis() > 16 || footer_time.as_millis() > 16 {
            let _ = config::log_message(&format!(
                "[RENDER DEBUG] mode={:?} header={}ms content={}ms footer={}ms",
                ctx.mode,
                header_time.as_millis(),
                content_time.as_millis(),
                footer_time.as_millis()
            ));
        }
    }
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Get mode-specific suffix (if any)
    let suffix = match ctx.mode {
        super::UiMode::Progress => None, // Never reached - handled separately
        super::UiMode::DirBrowser => Some("Directory Browser"),
        super::UiMode::DeploymentPreview => Some("Deployment Preview"),
        super::UiMode::ExitConfirmModal => Some("Exit Confirmation"),
        super::UiMode::CorpusBrowser => Some("Corpus Browser"),
        super::UiMode::Insights => Some("Corpus Insights"),
        super::UiMode::IntakeConfirmation => Some("Intake Confirmation"),
        super::UiMode::TagSearch => Some("Tag Search"),
        super::UiMode::UnifiedTagEditor => Some("Tag Editor"),
        super::UiMode::MissingFileResolution => Some("Missing File Resolution"),
        super::UiMode::TagCanonicityResolution => Some("Tag Canonicity"),
        super::UiMode::CompoundTagSplit => Some("Compound Tag Split"),
        super::UiMode::TransactionReview => Some("Transaction Review"),
        super::UiMode::FormatStandardization => Some("Format Standardization"),
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
    let start = Instant::now();
    let view_name: &str;

    match ctx.mode {
        super::UiMode::Progress => {
            // Never reached - handled separately in render() before this function
            view_name = "progress";
        }
        super::UiMode::DirBrowser => {
            view_name = "dir_browser";
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::DeploymentPreview => {
            view_name = "deployment_preview";
            if let Some(ref mut preview) = ctx.deployment_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::ExitConfirmModal => {
            view_name = "exit_confirm_modal";
            render_exit_confirm_modal(f, area, ctx.exit_confirm_modal_state);
        }
        super::UiMode::CorpusBrowser => {
            view_name = "corpus_browser";
            if let Some(ref mut browser) = ctx.tree_browser {
                browser.render(f, area);
            }
        }
        super::UiMode::Insights => {
            view_name = "insights";
            if let Some(ref mut view) = ctx.insights_view {
                insights_view::render_insights_view(f, area, view);
            } else {
                // Show loading state while insights view is initializing
                render_insights_loading(f, area);
            }
        }
        super::UiMode::TagSearch => {
            view_name = "tag_search";
            if let Some(ref state) = ctx.tag_search {
                state.render(f, area);
            }
        }
        super::UiMode::IntakeConfirmation => {
            view_name = "intake_confirmation";
            if let Some(ref state) = ctx.intake_confirmation {
                super::startup::intake_confirmation::render(f, area, state);
            }
        }
        super::UiMode::UnifiedTagEditor => {
            view_name = "unified_tag_editor";
            if let Some(ref mut editor) = ctx.unified_tag_editor {
                editor.render(f, area, ctx.status_message);
            }
        }
        super::UiMode::MissingFileResolution => {
            view_name = "missing_file_resolution";
            if let Some(ref preview) = ctx.missing_file_preview {
                preview.render(f, area);
            }
        }
        super::UiMode::TagCanonicityResolution => {
            view_name = "tag_canonicity_resolution";
            if let Some(ref state) = ctx.tag_canonicity_state {
                tag_canonicity::render(f, area, state);
            }
        }
        super::UiMode::CompoundTagSplit => {
            view_name = "compound_tag_split";
            if let Some(ref state) = ctx.compound_split_state {
                compound_split::render(f, area, state);
            }
        }
        super::UiMode::TransactionReview => {
            view_name = "transaction_review";
            if let Some(ref state) = ctx.transaction_review {
                transaction_review::render(f, area, state, &ctx.transaction_review_decisions);
            }
        }
        super::UiMode::FormatStandardization => {
            view_name = "format_standardization";
            if let Some(ref state) = ctx.format_std {
                format_standardization::render::render(f, area, state);
            }
        }
    }

    let elapsed = start.elapsed();
    if elapsed.as_millis() > 16 {
        let _ = config::log_message(&format!(
            "[RENDER DEBUG] render_content({}) took {}ms",
            view_name,
            elapsed.as_millis()
        ));
    }
}

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
            .fixed_size(50, 16)
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
            .fixed_size(40, 9)
            .style(ModalStyle::info())
            .centered()
            .render(f, area);
    }
}

fn render_footer(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Two-row layout: full-width corpus health on top, task/controls side-by-side below
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // Corpus health (slim)
            Constraint::Min(5),    // Task + Controls
        ])
        .split(area);

    // Bottom row: Task and Controls side-by-side
    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50), // Task
            Constraint::Percentage(50), // Controls
        ])
        .split(rows[1]);

    let start = Instant::now();
    render_corpus_status(f, rows[0], ctx);
    let corpus_time = start.elapsed();

    let start = Instant::now();
    render_operation_status(f, bottom_cols[0], ctx);
    let operation_time = start.elapsed();

    let start = Instant::now();
    render_controls(f, bottom_cols[1], ctx);
    let controls_time = start.elapsed();

    if corpus_time.as_millis() > 16
        || operation_time.as_millis() > 16
        || controls_time.as_millis() > 16
    {
        let _ = config::log_message(&format!(
            "[RENDER DEBUG] footer: corpus={}ms operation={}ms controls={}ms",
            corpus_time.as_millis(),
            operation_time.as_millis(),
            controls_time.as_millis()
        ));
    }
}

fn render_corpus_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    // Only split if db_stats is available (timing_instrumentation enabled)
    let (corpus_area, db_area) = if ctx.db_stats.is_some() {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        (split[0], Some(split[1]))
    } else {
        // No DB stats - give full width to corpus
        (area, None)
    };

    // Corpus stats
    let corpus_lines = if let Some(ref summary) = ctx.corpus_summary {
        let mut lines = Vec::new();

        // File-level stats: "N files (M indexed, Y missing, X new, Z relocated)"
        let mut file_parts: Vec<Span> = vec![
            Span::styled(
                summary.files_in_corpus.to_string(),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" files ("),
            Span::styled(
                summary.healthy_files.to_string(),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" indexed"),
        ];

        // Only show non-zero counts for file-level issues
        if summary.missing_files > 0 {
            file_parts.push(Span::raw(", "));
            file_parts.push(Span::styled(
                summary.missing_files.to_string(),
                Style::default().fg(Color::Red),
            ));
            file_parts.push(Span::raw(" missing"));
        }
        if summary.unindexed_files > 0 {
            file_parts.push(Span::raw(", "));
            file_parts.push(Span::styled(
                summary.unindexed_files.to_string(),
                Style::default().fg(Color::Yellow),
            ));
            file_parts.push(Span::raw(" new"));
        }
        if summary.moved_files > 0 {
            file_parts.push(Span::raw(", "));
            file_parts.push(Span::styled(
                summary.moved_files.to_string(),
                Style::default().fg(Color::Yellow),
            ));
            file_parts.push(Span::raw(" relocated"));
        }
        file_parts.push(Span::raw(")"));

        lines.push(Line::from(file_parts));

        // Signal breakdown (all remaining signals)
        let hs = &summary.signal_summary;
        let total_signals = hs.fingerprint_duplicates
            + hs.metadata_duplicates
            + hs.canonicalization_issues
            + hs.missing_tag_issues
            + summary.deploy_conflicts
            + summary.library_stale
            + summary.library_leftover
            + summary.modified_oob
            + summary.tags_changed_oob
            + summary.duplicate_inodes;

        if total_signals > 0 {
            let mut signal_parts: Vec<Span> = Vec::new();
            let mut first = true;

            // Helper macro to reduce repetition
            macro_rules! add_signal {
                ($count:expr, $label:expr, $color:expr) => {
                    if $count > 0 {
                        if !first { signal_parts.push(Span::raw(", ")); }
                        signal_parts.push(Span::styled(
                            $count.to_string(),
                            Style::default().fg($color),
                        ));
                        signal_parts.push(Span::raw(concat!(" ", $label)));
                        #[allow(unused_assignments)]
                        { first = false; }
                    }
                };
            }

            add_signal!(summary.library_stale, "stale", Color::Yellow);
            add_signal!(summary.library_leftover, "leftover", Color::Yellow);
            add_signal!(hs.missing_tag_issues, "missing-tags", Color::Yellow);
            add_signal!(summary.deploy_conflicts, "conflicts", Color::Red);
            add_signal!(hs.metadata_duplicates, "meta-dups", Color::Yellow);
            add_signal!(hs.fingerprint_duplicates, "fp-dups", Color::Yellow);
            add_signal!(hs.canonicalization_issues, "canon", Color::Yellow);
            add_signal!(summary.modified_oob, "modified-oob", Color::Yellow);
            add_signal!(summary.tags_changed_oob, "tags-oob", Color::Yellow);
            add_signal!(summary.duplicate_inodes, "dup-inodes", Color::Yellow);

            lines.push(Line::from(signal_parts));
        }

        lines
    } else {
        vec![Line::from("No data").style(Style::default().fg(Color::DarkGray))]
    };

    let corpus_para = Paragraph::new(corpus_lines)
        .block(Block::default().borders(Borders::ALL).title("Corpus"));
    f.render_widget(corpus_para, corpus_area);

    // Right: DB thread stats (only if timing_instrumentation is enabled)
    if let Some(db_area) = db_area {
        if let Some(ref stats) = ctx.db_stats {
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

            let db_lines = vec![
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
            ];

            let db_para = Paragraph::new(db_lines)
                .block(Block::default().borders(Borders::ALL).title("DB Thread"));
            f.render_widget(db_para, db_area);
        }
    }
}

fn render_operation_status(f: &mut Frame, area: ratatui::layout::Rect, ctx: &RenderContext) {
    let mut lines = Vec::new();

    let has_witch_work = ctx.witch_status.as_ref().map(|s| s.pending > 0).unwrap_or(false);
    let has_completed_session = ctx.witch_status.as_ref()
        .and_then(|s| s.completed_session.as_ref())
        .is_some();

    if has_witch_work {
        // Show active Witch status
        if let Some(ref status) = ctx.witch_status {
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
                Span::styled("The Witch ", Style::default().fg(Color::Cyan)),
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
        if let Some(ref status) = ctx.witch_status {
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
        super::UiMode::Progress => control_presets::empty(), // No controls during loading/analysis
        super::UiMode::DirBrowser => control_presets::dir_browser(),
        super::UiMode::DeploymentPreview => control_presets::deployment_preview(),
        super::UiMode::ExitConfirmModal => control_presets::exit_confirm_modal(),
        super::UiMode::CorpusBrowser => control_presets::corpus_browser(),
        super::UiMode::Insights => control_presets::insights_view(),
        super::UiMode::TagSearch => control_presets::tag_search(),
        super::UiMode::IntakeConfirmation => control_presets::empty(), // Modal handles its own hints
        super::UiMode::UnifiedTagEditor => control_presets::tag_editor(), // Reuse same controls
        super::UiMode::MissingFileResolution => control_presets::empty(), // Modal handles its own hints
        super::UiMode::TagCanonicityResolution => control_presets::empty(), // Modal handles its own hints
        super::UiMode::CompoundTagSplit => control_presets::empty(), // Modal handles its own hints
        super::UiMode::TransactionReview => control_presets::empty(), // Modal handles its own hints
        super::UiMode::FormatStandardization => control_presets::format_standardization(),
    };
    lines.push(controls.render_line());

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}

