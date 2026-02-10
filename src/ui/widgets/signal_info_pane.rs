//! Signal Info Pane Widget
//!
//! Context-sensitive information display for deploy signals.
//! Shows different information based on signal type.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::ui::helpers::render_pane;

use super::tabbed_signal_list::DeployTab;

/// Information about a deploy signal for display in the info pane.
#[derive(Debug, Clone, Default)]
pub enum SignalInfo {
    /// Healthy file: corpus path and library path match.
    Healthy {
        corpus_path: String,
        library_path: String,
    },
    /// New files ready to deploy: aggregated by directory.
    NewDirectory {
        directory: String,
        file_count: usize,
    },
    /// Deploy conflict: multiple corpus files map to same library path.
    Conflict {
        deploy_path: String,
        conflicting_files: Vec<String>,
    },
    /// Leftover files: aggregated by directory.
    LeftoverDirectory {
        directory: String,
        file_count: usize,
    },
    /// Stale file: library path differs from expected path.
    Stale {
        library_path: String,
        expected_path: String,
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
    info: &'a SignalInfo,
}

impl<'a> SignalInfoPane<'a> {
    pub fn new(active_tab: DeployTab, info: &'a SignalInfo) -> Self {
        Self { active_tab, info }
    }

    /// Render the info pane.
    pub fn render(self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title())
            .border_style(Style::default().fg(Color::White));

        let inner = render_pane(f, area, block);

        let lines = self.build_content();
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

    fn build_content(&self) -> Vec<Line<'a>> {
        match self.info {
            SignalInfo::None => self.no_selection_content(),
            SignalInfo::Healthy {
                corpus_path,
                library_path,
            } => self.healthy_content(corpus_path, library_path),
            SignalInfo::NewDirectory {
                directory,
                file_count,
            } => self.new_directory_content(directory, *file_count),
            SignalInfo::Conflict {
                deploy_path,
                conflicting_files,
            } => self.conflict_content(deploy_path, conflicting_files),
            SignalInfo::LeftoverDirectory {
                directory,
                file_count,
            } => self.leftover_directory_content(directory, *file_count),
            SignalInfo::Stale {
                library_path,
                expected_path,
            } => self.stale_content(library_path, expected_path),
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

    fn healthy_content(&self, corpus_path: &str, library_path: &str) -> Vec<Line<'a>> {
        vec![
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
            Line::from(corpus_path.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Library Path:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(library_path.to_string()),
        ]
    }

    fn new_directory_content(&self, directory: &str, file_count: usize) -> Vec<Line<'a>> {
        vec![
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
            Line::from(directory.to_string()),
            Line::from(""),
            Line::from(vec![
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
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Hard links will be created from corpus to library.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    }

    fn conflict_content(&self, deploy_path: &str, conflicting_files: &[String]) -> Vec<Line<'a>> {
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "CONFLICT - Cannot deploy",
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Target Library Path:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(deploy_path.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Conflicting Corpus Files:",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )),
        ];

        for file in conflicting_files {
            lines.push(Line::from(format!("  - {}", file)));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Resolve by editing tags to create unique paths.",
            Style::default().fg(Color::DarkGray),
        )));

        lines
    }

    fn leftover_directory_content(&self, directory: &str, file_count: usize) -> Vec<Line<'a>> {
        vec![
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
            Line::from(directory.to_string()),
            Line::from(""),
            Line::from(vec![
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
                Span::styled(" with no corpus backing", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "These files may be safe to remove if not needed.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    }

    fn stale_content(&self, library_path: &str, expected_path: &str) -> Vec<Line<'a>> {
        vec![
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
            Line::from(library_path.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Expected Library Path:",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(expected_path.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "The file's tags changed, requiring a new path.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "Re-deploying will move to the expected path.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    }
}
