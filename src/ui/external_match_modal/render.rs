//! Rendering for the external match review modal.
//!
//! Uses full-area layout with:
//! - Info bar showing full untruncated path and confidence
//! - 33% list pane / 67% details pane
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::meta::views::ExternalMatchClassificationView;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{render_file_path_list, FocusPane, PathEntry, PathField, ResolutionLayout};

use super::types::{ExternalMatchButton, ExternalMatchReviewState};

pub fn render(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    let layout = ResolutionLayout::new(padded, 3, 3, 33);

    render_info_bar(f, layout.info_bar, state);
    render_file_list(f, layout.list_pane, state);
    render_diff_details(f, layout.details_pane, state);
    render_buttons(f, layout.buttons, state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &ExternalMatchReviewState) {
    let title = format!(
        " External Match Review \u{2014} {} file{} ",
        state.entries.len(),
        if state.entries.len() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = render_pane(f, area, block);

    if let Some(entry) = state.entries.get(state.cursor) {
        let lines = PathField::new(
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            &entry.path,
        )
        .style(Style::default().fg(Color::White))
        .render_lines(inner.width);
        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }
}

fn render_file_list(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let is_focused = state.focus_pane == FocusPane::List;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .title("Files")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = render_pane(f, area, block);

    // Populate click targets for list items
    state.click_targets.clear();
    state.click_targets.set_list_area(inner);
    let visible_height = inner.height as usize;
    let scroll = state.scroll;
    for (vis_idx, entry_idx) in (scroll..).take(visible_height).enumerate() {
        if entry_idx >= state.entries.len() { break; }
        state.click_targets.add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
    }

    let entries: Vec<PathEntry> = state.entries
        .iter()
        .map(|entry| {
            let marker = match entry.classification {
                ExternalMatchClassificationView::ContentDiff => "!",
                ExternalMatchClassificationView::MetadataOnly => "?",
            };
            let marker_color = match entry.classification {
                ExternalMatchClassificationView::ContentDiff => Color::Yellow,
                ExternalMatchClassificationView::MetadataOnly => Color::Cyan,
            };

            PathEntry {
                path: &entry.path,
                prefix: vec![
                    Span::styled(
                        format!("{} ", marker),
                        Style::default().fg(marker_color),
                    ),
                ],
                suffix: vec![
                    Span::styled(
                        format!(" {:.0}%", entry.confidence * 100.0),
                        Style::default().fg(Color::DarkGray),
                    ),
                ],
            }
        })
        .collect();

    render_file_path_list(f, inner, &entries, state.cursor, state.scroll);
}

fn render_diff_details(f: &mut Frame, area: Rect, state: &ExternalMatchReviewState) {
    let block = Block::default()
        .title("Tag Differences")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = render_pane(f, area, block);

    let Some(entry) = state.entries.get(state.cursor) else {
        return;
    };

    let mut lines = Vec::new();

    // Confidence + recording ID header
    lines.push(Line::from(vec![
        Span::styled("Recording: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&entry.recording_id, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(
            format!("({:.0}% confidence)", entry.confidence * 100.0),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::raw(""));

    if entry.diffs.is_empty() {
        lines.push(Line::from(Span::styled(
            "No tag differences",
            Style::default().fg(Color::Green),
        )));
    } else {
        for diff in &entry.diffs {
            // Tag name label
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<10}", diff.tag_name),
                    Style::default().fg(Color::Cyan),
                ),
            ]));

            // External value
            lines.push(Line::from(vec![
                Span::styled("    ext:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("\"{}\"", diff.external_value),
                    Style::default().fg(Color::Green),
                ),
            ]));

            // Corpus value
            match &diff.corpus_value {
                Some(cv) => {
                    lines.push(Line::from(vec![
                        Span::styled("    corpus: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("\"{}\"", cv),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]));
                }
                None => {
                    lines.push(Line::from(vec![
                        Span::styled("    corpus: ", Style::default().fg(Color::DarkGray)),
                        Span::styled("[not in corpus]", Style::default().fg(Color::DarkGray)),
                    ]));
                }
            }

            lines.push(Line::raw(""));
        }
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn render_buttons(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let is_focused = state.focus_pane == FocusPane::Buttons;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));
    let inner = render_pane(f, area, block);

    state.button_rects.clear();

    let accept_label = " Accept Tags ";
    let dismiss_label = " Dismiss ";
    let cancel_label = " Cancel ";

    let total_width = accept_label.len() + 2 + dismiss_label.len() + 2 + cancel_label.len();
    let start_x = inner.x + (inner.width.saturating_sub(total_width as u16)) / 2;

    let mut x = start_x;

    let accept_rect = Rect::new(x, inner.y, accept_label.len() as u16, 1);
    state.button_rects.set("accept", accept_rect);
    x += accept_label.len() as u16 + 2;

    let dismiss_rect = Rect::new(x, inner.y, dismiss_label.len() as u16, 1);
    state.button_rects.set("dismiss", dismiss_rect);
    x += dismiss_label.len() as u16 + 2;

    let cancel_rect = Rect::new(x, inner.y, cancel_label.len() as u16, 1);
    state.button_rects.set("cancel", cancel_rect);

    let accept_style = if state.selected_button == ExternalMatchButton::Accept && is_focused {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else {
        Style::default().fg(Color::Green)
    };

    let dismiss_style = if state.selected_button == ExternalMatchButton::Dismiss && is_focused {
        Style::default().fg(Color::Black).bg(Color::Magenta)
    } else {
        Style::default().fg(Color::Magenta)
    };

    let cancel_style = if state.selected_button == ExternalMatchButton::Cancel && is_focused {
        Style::default().fg(Color::Black).bg(Color::White)
    } else {
        Style::default().fg(Color::White)
    };

    let buttons_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(accept_label, accept_style),
        Span::raw("  "),
        Span::styled(dismiss_label, dismiss_style),
        Span::raw("  "),
        Span::styled(cancel_label, cancel_style),
        Span::raw("  "),
    ]);

    let hint_style = Style::default().fg(Color::DarkGray);
    let hint_line = Line::from(vec![
        Span::styled("Shift+\u{2191}\u{2193}", hint_style),
        Span::styled(" focus", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("\u{2190}\u{2192}", hint_style),
        Span::styled(" buttons", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("Enter", hint_style),
        Span::styled(" confirm", hint_style),
    ]);

    let para = Paragraph::new(vec![buttons_line, hint_line]).alignment(Alignment::Center);
    f.render_widget(para, inner);
}
