//! Rendering for the external match review modal.
//!
//! Uses full-area layout with:
//! - Info bar showing full untruncated path and confidence
//! - 40% list pane / 60% details pane
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::meta::views::ExternalMatchClassificationView;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{render_file_path_list, FocusPane, PathEntry, PathField, ResolutionLayout, StyledCell, ThreeColTable};

use super::types::{ExternalMatchButton, ExternalMatchReviewState};

pub fn render(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    let layout = ResolutionLayout::new(padded, 3, 3, 40);

    render_info_bar(f, layout.info_bar, state);
    render_file_list(f, layout.list_pane, state);
    render_diff_details(f, layout.details_pane, state);
    render_buttons(f, layout.buttons, state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &ExternalMatchReviewState) {
    let sel_count = state.selection.selection_count();
    let title = if sel_count > 0 {
        format!(
            " External Match Review \u{2014} {} file{} ({} selected) ",
            state.entries.len(),
            if state.entries.len() == 1 { "" } else { "s" },
            sel_count,
        )
    } else {
        format!(
            " External Match Review \u{2014} {} file{} ",
            state.entries.len(),
            if state.entries.len() == 1 { "" } else { "s" },
        )
    };

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

    let selection_active = state.selection.is_active();
    let entries: Vec<PathEntry> = state.entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let marker = match entry.classification {
                ExternalMatchClassificationView::ContentDiff => "!",
                ExternalMatchClassificationView::MetadataOnly => "?",
            };
            let marker_color = match entry.classification {
                ExternalMatchClassificationView::ContentDiff => Color::Yellow,
                ExternalMatchClassificationView::MetadataOnly => Color::Cyan,
            };

            let mut prefix = Vec::new();
            if selection_active {
                let sel_marker = state.selection.marker(idx);
                let sel_color = if state.selection.is_selected(idx) { Color::Green } else { Color::DarkGray };
                prefix.push(Span::styled(
                    format!("{} ", sel_marker),
                    Style::default().fg(sel_color),
                ));
            }
            prefix.push(Span::styled(
                format!("{} ", marker),
                Style::default().fg(marker_color),
            ));

            PathEntry {
                path: &entry.path,
                prefix,
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

fn render_diff_details(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let block = Block::default()
        .title("Tag Differences")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = render_pane(f, area, block);

    let Some(entry) = state.entries.get(state.cursor) else {
        state.recording_link_rect = None;
        return;
    };

    let mut lines = Vec::new();

    // Confidence line
    lines.push(Line::from(Span::styled(
        format!("{:.0}% confidence", entry.confidence * 100.0),
        Style::default().fg(Color::DarkGray),
    )));

    // MusicBrainz recording URL (underlined for shift-click in terminal)
    let mb_url = format!("https://musicbrainz.org/recording/{}", entry.recording_id);
    let link_style = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    let link_len = mb_url.chars().count() as u16;

    // Track link position for mouse click → xdg-open
    let link_x = inner.x;
    let link_y = inner.y + 1; // second line
    state.recording_link_rect = Some(Rect::new(link_x, link_y, link_len, 1));

    lines.push(Line::from(Span::styled(&mb_url, link_style)));
    lines.push(Line::raw(""));

    if entry.diffs.is_empty() {
        lines.push(Line::from(Span::styled(
            "No tag differences",
            Style::default().fg(Color::Green),
        )));
        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    } else {
        let header_lines = lines.len() as u16;
        let para = Paragraph::new(lines);
        let header_area = Rect { height: header_lines, ..inner };
        f.render_widget(para, header_area);

        let table_area = Rect {
            y: inner.y + header_lines,
            height: inner.height.saturating_sub(header_lines),
            ..inner
        };

        let bold = Modifier::BOLD;
        let table = ThreeColTable {
            headers: [
                ("TAG".into(), Style::default().fg(Color::DarkGray).add_modifier(bold)),
                ("DISK".into(), Style::default().fg(Color::DarkGray).add_modifier(bold)),
                ("EXTERNAL".into(), Style::default().fg(Color::DarkGray).add_modifier(bold)),
            ],
            rows: entry.diffs.iter().map(|diff| {
                let disk_text = match &diff.corpus_value {
                    Some(cv) => format!("\"{}\"", cv),
                    None => "\u{2014}".to_string(),
                };
                let disk_color = match &diff.corpus_value {
                    Some(_) => Color::Red,
                    None => Color::DarkGray,
                };
                let ext_text = format!("\"{}\"", diff.external_value);
                [
                    StyledCell::new(&diff.tag_name, Style::default().fg(Color::Cyan)),
                    StyledCell::new(disk_text, Style::default().fg(disk_color)),
                    StyledCell::new(ext_text, Style::default().fg(Color::Green)),
                ]
            }).collect(),
            col_ratio: [20, 40, 40],
            scroll: state.diff_scroll,
            separator_style: Style::default().fg(Color::DarkGray),
            alternate_rows: true,
        };
        table.render(f, table_area);
    }
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
    let drop_label = " Drop Selected ";
    let dismiss_label = " Dismiss ";
    let cancel_label = " Cancel ";

    let total_width = accept_label.len() + 2 + drop_label.len() + 2 + dismiss_label.len() + 2 + cancel_label.len();
    let start_x = inner.x + (inner.width.saturating_sub(total_width as u16)) / 2;

    let mut x = start_x;

    let accept_rect = Rect::new(x, inner.y, accept_label.len() as u16, 1);
    state.button_rects.set("accept", accept_rect);
    x += accept_label.len() as u16 + 2;

    let drop_rect = Rect::new(x, inner.y, drop_label.len() as u16, 1);
    state.button_rects.set("drop_selected", drop_rect);
    x += drop_label.len() as u16 + 2;

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

    let drop_style = if state.selected_button == ExternalMatchButton::DropSelected && is_focused {
        Style::default().fg(Color::Black).bg(Color::Red)
    } else {
        Style::default().fg(Color::Red)
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
        Span::styled(drop_label, drop_style),
        Span::raw("  "),
        Span::styled(dismiss_label, dismiss_style),
        Span::raw("  "),
        Span::styled(cancel_label, cancel_style),
        Span::raw("  "),
    ]);

    let sel_count = state.selection.selection_count();
    let hint_style = Style::default().fg(Color::DarkGray);
    let mut hint_spans = vec![
        Span::styled("Space", hint_style),
        Span::styled(" select", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("Shift+\u{2191}\u{2193}", hint_style),
        Span::styled(" focus", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("\u{2190}\u{2192}", hint_style),
        Span::styled(" buttons", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("Enter", hint_style),
        Span::styled(" confirm", hint_style),
    ];
    if sel_count > 0 {
        hint_spans.push(Span::styled(
            format!("  \u{00b7}  {} selected", sel_count),
            Style::default().fg(Color::Yellow),
        ));
    }
    let hint_line = Line::from(hint_spans);

    let para = Paragraph::new(vec![buttons_line, hint_line]).alignment(Alignment::Center);
    f.render_widget(para, inner);
}
