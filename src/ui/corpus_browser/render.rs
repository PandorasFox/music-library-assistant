//! Corpus Browser Rendering

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::state::CorpusBrowserState;

impl CorpusBrowserState {
    /// Render the corpus browser.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Two-pane layout: 2/3 tree, 1/3 preview
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(65),
                Constraint::Percentage(35),
            ])
            .split(area);

        self.render_tree_pane(f, chunks[0]);
        self.render_preview_pane(f, chunks[1]);
    }

    /// Render the directory/file tree pane.
    fn render_tree_pane(&mut self, f: &mut Frame, area: Rect) {
        // Calculate visible height
        let visible_height = area.height.saturating_sub(2) as usize;
        self.set_visible_height(visible_height);

        // Build lines for visible entries
        let entries = self.entries();
        let cursor_idx = self.cursor_idx();
        let scroll = self.scroll_offset();

        let lines: Vec<Line> = entries
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible_height)
            .map(|(idx, entry)| {
                let is_current = idx == cursor_idx;

                // Build indentation
                let indent = "  ".repeat(entry.depth);

                // Expand/collapse indicator
                let expand_indicator = if entry.is_directory {
                    if entry.has_children {
                        if entry.is_expanded { "▼ " } else { "▶ " }
                    } else {
                        "  "
                    }
                } else {
                    "  "
                };

                // Item count for directories
                let count_suffix = if entry.is_directory && entry.item_count > 0 {
                    format!("  ({} tracks)", entry.item_count)
                } else {
                    String::new()
                };

                // Icon for files
                let icon = if entry.is_directory { "" } else { "♪ " };

                let line_text = format!(
                    "{}{}{}{}{}",
                    indent, expand_indicator, icon, entry.name, count_suffix
                );

                let style = if is_current {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else if entry.is_directory {
                    Style::default().fg(Color::Blue)
                } else {
                    Style::default()
                };

                Line::from(line_text).style(style)
            })
            .collect();

        // Scroll indicator
        let total_entries = entries.len();
        let scroll_indicator = if total_entries > visible_height {
            let pos = scroll + 1;
            let max = total_entries.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let tree_para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Files{}", scroll_indicator)),
        );
        f.render_widget(tree_para, area);
    }

    /// Render the metadata preview pane.
    fn render_preview_pane(&self, f: &mut Frame, area: Rect) {
        let mut lines = Vec::new();

        if let Some(entry) = self.current_entry() {
            if entry.is_directory {
                // Directory preview
                lines.push(Line::from(format!("Directory: {}", entry.name))
                    .style(Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)));
                lines.push(Line::from(""));
                lines.push(Line::from(format!("Path: {}", entry.path.display())));
                lines.push(Line::from(format!("Audio files: {}", entry.item_count)));
                lines.push(Line::from(""));
                lines.push(Line::from("Press Enter to edit tags")
                    .style(Style::default().fg(Color::DarkGray)));
            } else {
                // File preview
                lines.push(Line::from(format!("File: {}", entry.name))
                    .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
                lines.push(Line::from(""));

                if let Some(meta) = self.cached_metadata() {
                    // Technical info
                    lines.push(Line::from("─── Technical ───")
                        .style(Style::default().fg(Color::DarkGray)));

                    let bitrate = meta.bitrate_kbps
                        .map(|b| format!("{} kbps", b))
                        .unwrap_or_else(|| "Unknown".to_string());

                    let duration = meta.duration_ms
                        .map(|d| {
                            let secs = d / 1000;
                            format!("{}:{:02}", secs / 60, secs % 60)
                        })
                        .unwrap_or_else(|| "Unknown".to_string());

                    let sample_rate = meta.sample_rate
                        .map(|s| format!("{} Hz", s))
                        .unwrap_or_else(|| "Unknown".to_string());

                    let size = if meta.file_size < 1024 * 1024 {
                        format!("{:.1} KB", meta.file_size as f64 / 1024.0)
                    } else {
                        format!("{:.2} MB", meta.file_size as f64 / (1024.0 * 1024.0))
                    };

                    lines.push(Line::from(format!("Format: {}", meta.file_type)));
                    lines.push(Line::from(format!("Size: {}", size)));
                    lines.push(Line::from(format!("Bitrate: {}", bitrate)));
                    lines.push(Line::from(format!("Duration: {}", duration)));
                    lines.push(Line::from(format!("Sample Rate: {}", sample_rate)));

                    // Tags
                    if !meta.tags.is_empty() {
                        lines.push(Line::from(""));
                        lines.push(Line::from("─── Tags ───")
                            .style(Style::default().fg(Color::DarkGray)));

                        for (key, value) in &meta.tags {
                            let display_value = if value.len() > 30 {
                                format!("{}...", &value[..27])
                            } else {
                                value.clone()
                            };
                            lines.push(Line::from(format!("{:12}: {}", key, display_value)));
                        }
                    }

                    lines.push(Line::from(""));
                    lines.push(Line::from("Press Enter to edit tags")
                        .style(Style::default().fg(Color::DarkGray)));
                } else {
                    lines.push(Line::from("Loading metadata...")
                        .style(Style::default().fg(Color::DarkGray)));
                }
            }
        } else {
            lines.push(Line::from("No item selected")
                .style(Style::default().fg(Color::DarkGray)));
        }

        let preview_para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Preview"),
        );
        f.render_widget(preview_para, area);
    }
}
