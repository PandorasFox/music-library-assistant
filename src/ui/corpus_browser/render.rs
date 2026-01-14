//! Corpus Browser Rendering

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
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

        // Render overlays
        if self.is_match_selection_mode() {
            self.render_match_modal(f, area);
        } else if self.search().is_active() {
            self.render_search_popup(f, area);
        }
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
                            let display_value = crate::ui::helpers::truncate_right(value, 30);
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

    /// Render the search popup overlay.
    fn render_search_popup(&self, f: &mut Frame, area: Rect) {
        // Position popup near top of screen
        let popup_width = 50.min(area.width.saturating_sub(4));
        let popup_height = 3;
        let popup_x = (area.width.saturating_sub(popup_width)) / 2 + area.x;
        let popup_y = area.y + 2;

        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        // Clear the background
        f.render_widget(Clear, popup_area);

        // Build the search line with query and suggestion
        let query = &self.search().query;
        let suggestion = self.search().suggestion.as_deref().unwrap_or("");

        // Calculate the suggestion suffix (part after query)
        // Use char count to avoid slicing in the middle of multi-byte chars
        let suggestion_suffix = if !suggestion.is_empty()
            && suggestion.to_lowercase().starts_with(&query.to_lowercase())
        {
            let query_chars = query.chars().count();
            suggestion.chars().skip(query_chars).collect::<String>()
        } else {
            String::new()
        };

        let search_root_name = self
            .search()
            .search_root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".to_string());

        let match_count = self.search().matches.len();
        let match_info = if match_count == 0 {
            " (no matches)".to_string()
        } else if match_count == 1 {
            " (1 match)".to_string()
        } else {
            format!(" ({} matches)", match_count)
        };

        let line = Line::from(vec![
            Span::styled(query.as_str(), Style::default().fg(Color::White)),
            Span::styled(
                suggestion_suffix,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(
                match_info,
                Style::default().fg(if match_count > 0 {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
        ]);

        let title = format!("Search in: {}", search_root_name);
        let popup = Paragraph::new(line)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(title),
            )
            .alignment(Alignment::Left);

        f.render_widget(popup, popup_area);
    }

    /// Render the match selection modal.
    fn render_match_modal(&self, f: &mut Frame, area: Rect) {
        let matches = &self.search().matches;
        let selected_idx = self.match_selection_idx();

        // Calculate modal size
        let max_path_len = matches
            .iter()
            .map(|p| p.to_string_lossy().len())
            .max()
            .unwrap_or(20);

        let modal_width = (max_path_len as u16 + 6).min(area.width.saturating_sub(4));
        let modal_height = (matches.len() as u16 + 4).min(area.height.saturating_sub(4));

        let modal_x = (area.width.saturating_sub(modal_width)) / 2 + area.x;
        let modal_y = (area.height.saturating_sub(modal_height)) / 2 + area.y;

        let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

        // Clear background
        f.render_widget(Clear, modal_area);

        // Build match list
        let visible_height = modal_height.saturating_sub(4) as usize;
        let scroll_offset = if selected_idx >= visible_height {
            selected_idx - visible_height + 1
        } else {
            0
        };

        let lines: Vec<Line> = matches
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_height)
            .map(|(idx, path)| {
                let is_selected = idx == selected_idx;
                let prefix = if is_selected { "▶ " } else { "  " };

                // Truncate path if needed
                let path_str = path.to_string_lossy();
                let max_len = modal_width.saturating_sub(6) as usize;
                let display_path = crate::ui::helpers::truncate_left(&path_str, max_len);

                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                Line::from(format!("{}{}", prefix, display_path)).style(style)
            })
            .collect();

        let title = format!(
            "Select Match ({}/{})",
            selected_idx + 1,
            matches.len()
        );

        let modal = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(title),
        );

        f.render_widget(modal, modal_area);

        // Render help text at bottom of modal
        if modal_height > 3 {
            let help_area = Rect::new(
                modal_x + 1,
                modal_y + modal_height - 2,
                modal_width - 2,
                1,
            );
            let help = Paragraph::new(Line::from(vec![
                Span::styled("↑↓", Style::default().fg(Color::Cyan)),
                Span::raw(" navigate  "),
                Span::styled("Enter", Style::default().fg(Color::Cyan)),
                Span::raw(" select  "),
                Span::styled("Esc", Style::default().fg(Color::Cyan)),
                Span::raw(" cancel"),
            ]))
            .alignment(Alignment::Center);
            f.render_widget(help, help_area);
        }
    }
}
