//! Deployment Preview UI
//!
//! Shows a tabbed view of deploy signals with an info pane.
//! - Tab navigation with Left/Right arrows
//! - Scrollable list of signals sorted by path (non-interactable)
//! - Info pane showing context for selected signal type
//! - Enter stages mutations and goes directly to TransactionReview
//! - Escape returns to Insights view

use std::borrow::Cow;
use std::path::Path;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::DeployModalData;
use crate::widgets::{DeployTab, SidecarSummary, SignalInfo, SignalInfoPane, TabbedSignalList};

/// State for the deployment preview modal.
///
/// After the data/interaction split, this type is retained only for
/// its static helper methods that render and query cached data.
/// `active_tab` and `tab_scroll` live in `DeployInteraction` (mm-ui).
#[derive(Debug)]
pub struct DeploymentPreviewState;

impl DeploymentPreviewState {
    /// Determine which tab to start on based on data content.
    pub fn initial_tab(cached_data: &DeployModalData) -> DeployTab {
        if !cached_data.new.is_empty() {
            DeployTab::New
        } else {
            DeployTab::Healthy
        }
    }

    /// Path of the currently selected item (for status bar).
    pub fn selected_path_static(
        cached_data: &DeployModalData,
        active_tab: DeployTab,
        scroll: usize,
    ) -> Option<&str> {
        match active_tab {
            DeployTab::Healthy => cached_data
                .healthy
                .get(scroll)
                .map(|f| f.corpus_path.as_str()),
            DeployTab::New => cached_data
                .new_by_dir
                .get(scroll)
                .map(|d| d.directory.as_str()),
            DeployTab::Conflicts => cached_data
                .conflicts
                .get(scroll)
                .map(|c| c.deploy_path.as_str()),
            DeployTab::Leftover => cached_data
                .leftover_by_dir
                .get(scroll)
                .map(|d| d.directory.as_str()),
            DeployTab::Stale => cached_data
                .stale
                .get(scroll)
                .map(|f| f.library_path.as_str()),
        }
    }

    /// Render the deployment preview with externally-owned tab/scroll state.
    pub fn render_static(
        cached_data: &DeployModalData,
        active_tab: DeployTab,
        tab_scroll: [usize; 5],
        f: &mut Frame,
        area: Rect,
    ) {
        // Clear background
        f.render_widget(Clear, area);

        // Title height: 3 normally, 4 with per-library subtitle
        let title_height = if cached_data.per_library.is_empty() {
            3
        } else {
            4
        };

        // Layout: title + main content + controls
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(title_height),
                Constraint::Min(10),   // Content
                Constraint::Length(2), // Controls hint
            ])
            .split(area);

        // Render title
        render_title(cached_data, f, main_chunks[0]);

        // Render main content (tabbed list + info pane)
        render_content(cached_data, active_tab, tab_scroll, f, main_chunks[1]);

        // Render controls hint
        render_controls(f, main_chunks[2]);
    }
}

fn render_title(data: &DeployModalData, f: &mut Frame, area: Rect) {
    let total_ops = data.total_operations();

    // Build summary line
    let mut summary_parts: Vec<String> = Vec::new();
    if !data.new.is_empty() {
        summary_parts.push(format!("{} new", data.new.len()));
    }
    if !data.sidecars.is_empty() {
        summary_parts.push(format!("{} covers", data.sidecars.len()));
    }
    if !data.leftover.is_empty() {
        if data.replaced_count > 0 {
            summary_parts.push(format!(
                "{} leftover ({} replaced)",
                data.leftover.len(),
                data.replaced_count
            ));
        } else {
            summary_parts.push(format!("{} leftover", data.leftover.len()));
        }
    }
    if !data.stale.is_empty() {
        summary_parts.push(format!("{} stale", data.stale.len()));
    }
    let total_conflicts = data.conflicts.len() + data.sidecar_conflicts.len();
    if total_conflicts > 0 {
        summary_parts.push(format!("{} conflicts", total_conflicts));
    }

    let summary_str = if summary_parts.is_empty() {
        format!("({} total operations)", total_ops)
    } else {
        format!("({} — {})", total_ops, summary_parts.join(", "))
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            " Deploy Preview ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", summary_str),
            Style::default().fg(Color::DarkGray),
        ),
    ])];

    // Per-library subtitle when multiple libraries
    if !data.per_library.is_empty() {
        let lib_parts: Vec<String> = data
            .per_library
            .iter()
            .map(|lib| {
                let mut parts = Vec::new();
                if lib.new_count > 0 {
                    parts.push(format!("{}n", lib.new_count));
                }
                if lib.leftover_count > 0 {
                    if lib.replaced_count > 0 {
                        parts.push(format!("{}l({}r)", lib.leftover_count, lib.replaced_count));
                    } else {
                        parts.push(format!("{}l", lib.leftover_count));
                    }
                }
                if lib.stale_count > 0 {
                    parts.push(format!("{}s", lib.stale_count));
                }
                format!("{}: {}", lib.library_name, parts.join(", "))
            })
            .collect();

        lines.push(Line::from(Span::styled(
            format!(" {}", lib_parts.join("  |  ")),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));

    f.render_widget(paragraph, area);
}

fn render_content(
    cached_data: &DeployModalData,
    active_tab: DeployTab,
    tab_scroll: [usize; 5],
    f: &mut Frame,
    area: Rect,
) {
    // Split: tabbed list (60%) + info pane (40%)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    // Render tabbed list on left
    render_tabbed_list(cached_data, active_tab, tab_scroll, f, chunks[0]);

    // Render info pane on right
    render_info_pane(cached_data, active_tab, tab_scroll, f, chunks[1]);
}

fn render_tabbed_list(
    cached_data: &DeployModalData,
    active_tab: DeployTab,
    tab_scroll: [usize; 5],
    f: &mut Frame,
    area: Rect,
) {
    // Get display items for current tab
    // For New and Leftover, show "directory (count)" format
    let items: Vec<Cow<'_, str>> = match active_tab {
        DeployTab::Healthy => cached_data
            .healthy
            .iter()
            .map(|f| Cow::Borrowed(f.corpus_path.as_str()))
            .collect(),
        DeployTab::New => cached_data
            .new_by_dir
            .iter()
            .map(|d| {
                Cow::Owned(if d.count == 0 {
                    // Sidecar-only directory
                    format!("{} ({} covers)", d.directory, d.sidecar_count)
                } else if d.sidecar_count > 0 {
                    format!("{} ({} + {} covers)", d.directory, d.count, d.sidecar_count)
                } else {
                    format!("{} ({})", d.directory, d.count)
                })
            })
            .collect(),
        DeployTab::Conflicts => {
            let mut items: Vec<Cow<'_, str>> = cached_data
                .conflicts
                .iter()
                .map(|g| Cow::Borrowed(g.deploy_path.as_str()))
                .collect();
            items.extend(
                cached_data
                    .sidecar_conflicts
                    .iter()
                    .map(|g| Cow::Owned(format!("[img] {}/{}", g.library_name, g.deploy_path))),
            );
            items
        }
        DeployTab::Leftover => cached_data
            .leftover_by_dir
            .iter()
            .map(|d| Cow::Owned(format!("{} ({})", d.directory, d.count)))
            .collect(),
        DeployTab::Stale => cached_data
            .stale
            .iter()
            .map(|f| Cow::Borrowed(f.library_path.as_str()))
            .collect(),
    };

    let paths: Vec<&str> = items.iter().map(|s| s.as_ref()).collect();
    let scroll = tab_scroll[active_tab.index()];

    let widget = TabbedSignalList::new(active_tab)
        .items(paths)
        .scroll(scroll)
        .tab_counts(cached_data.tab_counts());

    widget.render(f, area);
}

fn render_info_pane(
    cached_data: &DeployModalData,
    active_tab: DeployTab,
    tab_scroll: [usize; 5],
    f: &mut Frame,
    area: Rect,
) {
    let scroll = tab_scroll[active_tab.index()];

    // Sidecar conflicts need an owned deploy_path from format!(), so we
    // hold it here to keep it alive for the SignalInfo borrow.
    let mut owned_deploy_path = String::new();

    let info = match active_tab {
        DeployTab::Healthy => {
            cached_data
                .healthy
                .get(scroll)
                .map(|file| SignalInfo::Healthy {
                    corpus_path: &file.corpus_path,
                    library_path: &file.deploy_path,
                })
        }
        DeployTab::New => {
            cached_data.new_by_dir.get(scroll).map(|dir| {
                // Find sidecars matching this directory
                let sidecars: Vec<SidecarSummary<'_>> = cached_data
                    .sidecars
                    .iter()
                    .filter(|s| {
                        Path::new(&s.corpus_image_path)
                            .parent()
                            .map(|p| p.to_string_lossy())
                            .is_some_and(|p| p == dir.directory)
                    })
                    .map(|s| SidecarSummary {
                        filename: &s.filename,
                        format: &s.format,
                        width: s.width,
                        height: s.height,
                        role: &s.role,
                    })
                    .collect();

                SignalInfo::NewDirectory {
                    directory: &dir.directory,
                    file_count: dir.count,
                    sidecars,
                }
            })
        }
        DeployTab::Conflicts => {
            let audio_len = cached_data.conflicts.len();
            if scroll < audio_len {
                cached_data
                    .conflicts
                    .get(scroll)
                    .map(|group| SignalInfo::Conflict {
                        deploy_path: Cow::Borrowed(&group.deploy_path),
                        conflicting_files: group
                            .conflicting_files
                            .iter()
                            .map(|(path, _)| path.as_str())
                            .collect(),
                    })
            } else {
                cached_data
                    .sidecar_conflicts
                    .get(scroll - audio_len)
                    .map(|group| {
                        owned_deploy_path =
                            format!("{}/{}", group.library_name, group.deploy_path);
                        SignalInfo::Conflict {
                            deploy_path: Cow::Borrowed(owned_deploy_path.as_str()),
                            conflicting_files: group
                                .conflicting_files
                                .iter()
                                .map(|(path, _)| path.as_str())
                                .collect(),
                        }
                    })
            }
        }
        DeployTab::Leftover => cached_data.leftover_by_dir.get(scroll).map(|dir| {
            SignalInfo::LeftoverDirectory {
                directory: &dir.directory,
                file_count: dir.count,
            }
        }),
        DeployTab::Stale => cached_data
            .stale
            .get(scroll)
            .map(|file| SignalInfo::Stale {
                library_path: &file.library_path,
                expected_path: &file.expected_path,
            }),
    };

    let info = info.unwrap_or(SignalInfo::None);
    let pane = SignalInfoPane::new(active_tab, &info);
    pane.render(f, area);
}

fn render_controls(f: &mut Frame, area: Rect) {
    let controls =
        "[Tab/Arrows] Switch Tab  [Up/Down] Scroll  [Enter] Stage & Review  [Esc] Cancel";

    let paragraph = Paragraph::new(controls)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));

    f.render_widget(paragraph, area);
}
