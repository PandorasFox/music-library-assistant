//! Filter Popup Rendering
//!
//! Renders the filter popup overlay as a centered modal.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::centered_rect_fixed;

use super::state::{FilterConditionType, FilterFieldFocus, FilterPopupState};

/// Render the filter popup overlay.
pub fn render(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    // Tag conditions need more height for the extra fields
    let height = match state.condition.condition_type {
        FilterConditionType::Tag => 14,
        _ => 12,
    };

    // Use a fixed-size centered popup
    let popup_area = centered_rect_fixed(55, height, area);

    // Clear the background
    f.render_widget(Clear, popup_area);

    // Main block
    let block = Block::default()
        .title(" Filter Files ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    // Layout depends on condition type - Tag needs more rows
    let constraints = match state.condition.condition_type {
        FilterConditionType::Tag => vec![
            Constraint::Length(1), // Instructions
            Constraint::Length(1), // Spacing
            Constraint::Length(1), // Condition type
            Constraint::Length(1), // Spacing
            Constraint::Length(1), // Tag name
            Constraint::Length(1), // Tag comparison + value
            Constraint::Length(1), // Spacing
            Constraint::Length(1), // Buttons
            Constraint::Min(0),    // Remaining space
        ],
        _ => vec![
            Constraint::Length(1), // Instructions
            Constraint::Length(1), // Spacing
            Constraint::Length(1), // Condition type
            Constraint::Length(1), // Spacing
            Constraint::Length(1), // Value field(s)
            Constraint::Length(1), // Spacing
            Constraint::Length(1), // Buttons
            Constraint::Min(0),    // Remaining space
        ],
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Instructions
    let instructions = Paragraph::new("Use Tab/arrows to navigate, Left/Right to cycle options")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(instructions, chunks[0]);

    // Condition type selector
    render_condition_type(f, chunks[2], state);

    // Value fields (depends on condition type)
    match state.condition.condition_type {
        FilterConditionType::Tag => {
            render_tag_name_field(f, chunks[4], state);
            render_tag_value_field(f, chunks[5], state);
            render_buttons(f, chunks[7], state);
        }
        _ => {
            render_value_fields(f, chunks[4], state);
            render_buttons(f, chunks[6], state);
        }
    }
}

fn render_condition_type(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let is_focused = state.focus == FilterFieldFocus::ConditionType;
    let style = if is_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let label = match state.condition.condition_type {
        FilterConditionType::Path => "Path",
        FilterConditionType::FileType => "File Type",
        FilterConditionType::SampleRate => "Sample Rate (Hz)",
        FilterConditionType::Bitrate => "Bitrate (kbps)",
        FilterConditionType::Duration => "Duration (seconds)",
        FilterConditionType::Tag => "Tag",
    };

    let line = Line::from(vec![
        Span::styled("Filter by: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("< {} >", label), style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}

fn render_value_fields(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    match state.condition.condition_type {
        FilterConditionType::Path => render_path_field(f, area, state),
        FilterConditionType::FileType => render_file_type_field(f, area, state),
        FilterConditionType::SampleRate | FilterConditionType::Bitrate | FilterConditionType::Duration => {
            render_range_fields(f, area, state)
        }
        FilterConditionType::Tag => {
            // Tag fields are rendered separately in main render function
        }
    }
}

fn render_path_field(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let is_focused = state.focus == FilterFieldFocus::PathSubstring;
    let style = if is_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let value_display = if state.condition.path_substring.is_empty() {
        "type to filter...".to_string()
    } else {
        state.condition.path_substring.value().to_string()
    };

    let value_style = if state.condition.path_substring.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        style
    };

    let line = Line::from(vec![
        Span::styled("Contains: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("[{}]", value_display), value_style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}

fn render_tag_name_field(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let is_focused = state.focus == FilterFieldFocus::TagName;
    let style = if is_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let line = Line::from(vec![
        Span::styled("Tag: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("< {} >", state.condition.tag_name), style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}

fn render_tag_value_field(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let comp_focused = state.focus == FilterFieldFocus::TagComparison;
    let value_focused = state.focus == FilterFieldFocus::TagValue;

    let comp_style = if comp_focused {
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Magenta)
    };

    let value_style = if value_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let value_display = if state.condition.tag_value.is_empty() {
        "type value...".to_string()
    } else {
        state.condition.tag_value.value().to_string()
    };

    let actual_value_style = if state.condition.tag_value.is_empty() && !value_focused {
        Style::default().fg(Color::DarkGray)
    } else {
        value_style
    };

    let line = Line::from(vec![
        Span::styled(format!("< {} >", state.condition.tag_comparison.label()), comp_style),
        Span::raw(" "),
        Span::styled(format!("[{}]", value_display), actual_value_style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}

fn render_file_type_field(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let is_focused = state.focus == FilterFieldFocus::FileTypeCategory;
    let style = if is_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let label = state.condition.file_type_category.label();

    let line = Line::from(vec![
        Span::styled("Category: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("< {} >", label), style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}

fn render_range_fields(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let min_focused = state.focus == FilterFieldFocus::RangeMin;
    let max_focused = state.focus == FilterFieldFocus::RangeMax;

    let min_style = if min_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let max_style = if max_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let min_display = if state.condition.range_min.is_empty() {
        "_____".to_string()
    } else {
        format!("{:>5}", state.condition.range_min.value())
    };

    let max_display = if state.condition.range_max.is_empty() {
        "_____".to_string()
    } else {
        format!("{:<5}", state.condition.range_max.value())
    };

    let line = Line::from(vec![
        Span::styled("Min: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("[{}]", min_display), min_style),
        Span::raw("  "),
        Span::styled("Max: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("[{}]", max_display), max_style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}

fn render_buttons(f: &mut Frame, area: Rect, state: &FilterPopupState) {
    let apply_focused = state.focus == FilterFieldFocus::ApplyButton;
    let clear_focused = state.focus == FilterFieldFocus::ClearButton;

    let apply_style = if apply_focused {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else {
        Style::default().fg(Color::Green)
    };

    let clear_style = if clear_focused {
        Style::default().fg(Color::Black).bg(Color::Red)
    } else {
        Style::default().fg(Color::Red)
    };

    let line = Line::from(vec![
        Span::styled(" Apply ", apply_style),
        Span::raw("   "),
        Span::styled(" Clear ", clear_style),
    ]);

    let para = Paragraph::new(line).alignment(Alignment::Center);
    f.render_widget(para, area);
}
