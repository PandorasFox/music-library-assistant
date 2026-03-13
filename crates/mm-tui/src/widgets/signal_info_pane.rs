//! Signal Info Pane Widget
//!
//! Context-sensitive information display for deploy signals.
//! Shows different information based on signal type.

use std::borrow::Cow;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::helpers::render_pane;

use super::path_display::{path_lines, PathField};
use super::tabbed_signal_list::DeployTab;

/// Summary of a sidecar image for the info pane.
#[derive(Debug, Clone)]
pub struct SidecarSummary<'a> {
    pub filename: &'a str,
    pub format: &'a str,
    pub width: u32,
    pub height: u32,
    pub role: &'a str,
}

/// Information about a deploy signal for display in the info pane.
#[derive(Debug, Clone, Default)]
pub enum SignalInfo<'a> {
    /// Healthy file: corpus path and library path match.
    Healthy {
        corpus_path: &'a str,
        library_path: &'a str,
    },
    /// New files ready to deploy: aggregated by directory.
    NewDirectory {
        directory: &'a str,
        file_count: usize,
        sidecars: Vec<SidecarSummary<'a>>,
    },
    /// Deploy conflict: multiple corpus files map to same library path.
    Conflict {
        deploy_path: Cow<'a, str>,
        conflicting_files: Vec<&'a str>,
    },
    /// Leftover files: aggregated by directory.
    LeftoverDirectory {
        directory: &'a str,
        file_count: usize,
    },
    /// Stale file: library path differs from expected path.
    Stale {
        library_path: &'a str,
        expected_path: &'a str,
    },
    /// No selection.
    #[default]
    None,
}

/// Info pane widget showing contextual information about a selected signal.
pub struct SignalInfoPane<'a> {
    /// The active tab determines the type of info shown.
    active_tab: DeployTab,
    /// Current signal info to display.
    info: &'a SignalInfo<'a>,
}

impl<'a> SignalInfoPane<'a> {
    pub fn new(active_tab: DeployTab, info: &'a SignalInfo<'a>) -> Self {
        Self { active_tab, info }
    }

    /// Render the info pane.
    pub fn render(self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title())
            .border_style(Style::default().fg(Color::White));

        let inner = render_pane(f, area, block);

        let lines = self.build_content(inner.width);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        f.render_widget(paragraph, inner);
    }

    fn title(&self) -> &'static str {
        match self.active_tab {
            DeployTab::Healthy => "Healthy File Info",
            DeployTab::New => "New Directory Info",
            DeployTab::Conflicts => "Conflict Info",
            DeployTab::Leftover => "Leftover Directory Info",
            DeployTab::Stale => "Stale File Info",
        }
    }

    fn build_content(&self, width: u16) -> Vec<Line<'a>> {
        match self.info {
            SignalInfo::None => self.no_selection_content(),
            SignalInfo::Healthy {
                corpus_path,
                library_path,
            } => self.healthy_content(corpus_path, library_path, width),
            SignalInfo::NewDirectory {
                directory,
                file_count,
                sidecars,
            } => self.new_directory_content(directory, *file_count, sidecars, width),
            SignalInfo::Conflict {
                deploy_path,
                conflicting_files,
            } => self.conflict_content(deploy_path.as_ref(), conflicting_files, width),
            SignalInfo::LeftoverDirectory {
                directory,
                file_count,
            } => self.leftover_directory_content(directory, *file_count, width),
            SignalInfo::Stale {
                library_path,
                expected_path,
            } => self.stale_content(library_path, expected_path, width),
        }
    }

    fn no_selection_content(&self) -> Vec<Line<'a>> {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "No file selected.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Use Left/Right to switch tabs.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    }

    fn healthy_content(&self, corpus_path: &str, library_path: &str, width: u16) -> Vec<Line<'a>> {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Deployed correctly",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Corpus Path:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        lines.extend(path_lines(corpus_path, Style::default(), width));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Library Path:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.extend(path_lines(library_path, Style::default(), width));
        lines
    }

    fn new_directory_content(
        &self,
        directory: &str,
        file_count: usize,
        sidecars: &[SidecarSummary<'a>],
        width: u16,
    ) -> Vec<Line<'a>> {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Ready to deploy",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Directory:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        lines.extend(path_lines(directory, Style::default(), width));
        lines.push(Line::from(""));
        if file_count > 0 {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}", file_count),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if file_count == 1 { " file" } else { " files" },
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(" ready to deploy", Style::default().fg(Color::DarkGray)),
            ]));
        }

        if !sidecars.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Sidecar Images ({}):", sidecars.len()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            for sc in sidecars {
                let role_suffix = if sc.role == "cover_front" {
                    " [front]"
                } else if sc.role == "cover_back" {
                    " [back]"
                } else {
                    ""
                };
                let desc = format!(
                    "  {} {}x{} {}{}",
                    sc.filename,
                    sc.width,
                    sc.height,
                    sc.format.to_uppercase(),
                    role_suffix
                );
                lines.push(Line::from(Span::styled(
                    desc,
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Hard links will be created from corpus to library.",
            Style::default().fg(Color::DarkGray),
        )));
        lines
    }

    fn conflict_content(
        &self,
        deploy_path: &str,
        conflicting_files: &[&str],
        width: u16,
    ) -> Vec<Line<'a>> {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "CONFLICT - Cannot deploy",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Target Library Path:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        lines.extend(path_lines(deploy_path, Style::default(), width));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Conflicting Corpus Files:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));

        for file in conflicting_files {
            lines.extend(PathField::new(Span::raw("  - "), file).render_lines(width));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Resolve by editing tags to create unique paths.",
            Style::default().fg(Color::DarkGray),
        )));

        lines
    }

    fn leftover_directory_content(
        &self,
        directory: &str,
        file_count: usize,
        width: u16,
    ) -> Vec<Line<'a>> {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Orphaned library files",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Directory:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        lines.extend(path_lines(directory, Style::default(), width));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}", file_count),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if file_count == 1 { " file" } else { " files" },
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                " with no corpus backing",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "These files may be safe to remove if not needed.",
            Style::default().fg(Color::DarkGray),
        )));
        lines
    }

    fn stale_content(&self, library_path: &str, expected_path: &str, width: u16) -> Vec<Line<'a>> {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Path outdated (tags changed)",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Current Library Path:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        lines.extend(path_lines(library_path, Style::default(), width));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Expected Library Path:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
        lines.extend(path_lines(expected_path, Style::default(), width));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "The file's tags changed, requiring a new path.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "Re-deploying will move to the expected path.",
            Style::default().fg(Color::DarkGray),
        )));
        lines
    }
}
