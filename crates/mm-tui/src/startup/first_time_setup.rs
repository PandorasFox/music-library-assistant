//! First-Time Setup UI Components
//!
//! Provides the interactive UI elements for first-time setup, called from
//! `run_tui()` when the Witch is in `AwaitingSetup` state:
//!
//! - `run_directory_picker()`: Interactive filesystem browser for selecting archive root.
//! - `run_create_account()`: Form for creating the first administrator account.
//!
//! Infrastructure creation (config, dirs, DB) is handled by the Witch via `CompleteSetup`.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::path::{Path, PathBuf};

use crate::tree_browser::{EntryFilter, TreeEntry, TreeNavigator};

// Re-export from mm-meta where the canonical definition lives.

use crate::input;
use crate::widgets::selection_styles::CURSOR_STYLE;
use crate::widgets::TextInputState;

// ============================================================================
// Directory Picker
// ============================================================================

/// State for the directory picker step.
struct DirectoryPickerState {
    navigator: TreeNavigator,
    /// Active inline directory creation (None = normal navigation mode)
    creating_dir: Option<CreateDirState>,
    /// Transient status message shown at bottom
    status_message: Option<String>,
}

struct CreateDirState {
    parent_path: PathBuf,
    input: TextInputState,
}

impl DirectoryPickerState {
    fn new() -> Self {
        let start_path = PathBuf::from("/");
        let filter = EntryFilter::directories_only();
        let mut navigator = TreeNavigator::new(start_path, filter, false, vec![], vec![]);
        navigator.show_new_dir_entry = true;

        Self {
            navigator,
            creating_dir: None,
            status_message: None,
        }
    }

    /// Recreate the navigator (after creating a directory) and navigate to a path.
    fn refresh_and_navigate_to(&mut self, target: &Path) {
        let start_path = PathBuf::from("/");
        let filter = EntryFilter::directories_only();
        let mut navigator = TreeNavigator::new(start_path, filter, false, vec![], vec![]);
        navigator.show_new_dir_entry = true;
        navigator.navigate_to_path(target);
        self.navigator = navigator;
    }
}

/// Run the directory picker, returning the selected path.
///
/// Called from `run_tui()` during first-time setup. The caller (TUI) owns the terminal.
/// If `suggested_root` is provided (e.g. from MM_ROOT env var), the picker
/// navigates there initially.
pub fn run_directory_picker<B: Backend>(
    terminal: &mut Terminal<B>,
    suggested_root: Option<PathBuf>,
) -> Result<PathBuf> {
    let mut state = DirectoryPickerState::new();
    if let Some(ref root) = suggested_root {
        state.navigator.navigate_to_path(root);
    }

    loop {
        terminal.draw(|f| render_directory_picker(f, &mut state))?;

        if let Event::Key(key) = event::read()? {
            if let Some(ref mut creating) = state.creating_dir {
                // Creating-directory mode: text input captures keys
                match key.code {
                    KeyCode::Enter => {
                        let name = creating.input.value().trim().to_string();
                        let parent = creating.parent_path.clone();
                        if !name.is_empty() && !name.contains('/') {
                            let new_path = parent.join(&name);
                            match std::fs::create_dir(&new_path) {
                                Ok(()) => {
                                    state.status_message =
                                        Some(format!("Created: {}", new_path.display()));
                                    state.creating_dir = None;
                                    state.refresh_and_navigate_to(&new_path);
                                }
                                Err(e) => {
                                    state.status_message = Some(format!("Error: {}", e));
                                    state.creating_dir = None;
                                }
                            }
                        } else {
                            state.creating_dir = None;
                        }
                    }
                    KeyCode::Esc => {
                        state.creating_dir = None;
                    }
                    _ => {
                        creating.input.handle_input(&input::map_key(key));
                    }
                }
            } else {
                // Normal navigation mode
                match key.code {
                    KeyCode::Up => {
                        state.navigator.move_up();
                        state.status_message = None;
                    }
                    KeyCode::Down => {
                        state.navigator.move_down();
                        state.status_message = None;
                    }
                    KeyCode::Right => {
                        state.navigator.expand_current();
                    }
                    KeyCode::Left => {
                        state.navigator.collapse_or_parent();
                    }
                    KeyCode::Enter => {
                        if let Some(entry) = state.navigator.current_entry().cloned() {
                            if entry.is_synthetic {
                                // Activate inline directory creation
                                state.creating_dir = Some(CreateDirState {
                                    parent_path: entry.path.clone(),
                                    input: TextInputState::new(),
                                });
                            } else if entry.is_directory() {
                                // Select this directory as archive root
                                return Ok(entry.path);
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        // Create new directory inside the currently selected directory
                        if let Some(entry) = state.navigator.current_entry().cloned() {
                            if entry.is_directory() && !entry.is_synthetic {
                                let parent = entry.path.clone();
                                // Expand directory if collapsed
                                if !entry.is_expanded {
                                    state.navigator.expand_current();
                                }
                                // Move cursor down to the [+ new directory] entry
                                state.navigator.move_down();
                                state.creating_dir = Some(CreateDirState {
                                    parent_path: parent,
                                    input: TextInputState::new(),
                                });
                            }
                        }
                    }
                    KeyCode::Esc => {
                        return Err(anyhow::anyhow!("Setup cancelled by user"));
                    }
                    _ => {}
                }
            }
        }
    }
}

// ============================================================================
// Step 1: Directory Picker Rendering
// ============================================================================

fn render_directory_picker(f: &mut ratatui::Frame, state: &mut DirectoryPickerState) {
    let area = f.area();

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" First-Time Setup \u{2014} Step 1 of 3 ")
        .title_alignment(Alignment::Center);

    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7), // Description
            Constraint::Min(5),    // Tree browser
            Constraint::Length(1), // Controls hint
        ])
        .split(inner);

    // Description
    let desc_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " Select a read/write directory for your music archive root.",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " This directory should be empty. MM will create subdirectories:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            Span::styled("   corpus/", Style::default().fg(Color::Yellow)),
            Span::styled("  libraries/", Style::default().fg(Color::Yellow)),
            Span::styled("  stash/", Style::default().fg(Color::Yellow)),
            Span::styled("  inbox/", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
    ];
    f.render_widget(Paragraph::new(desc_lines), chunks[0]);

    // Tree browser
    render_tree(f, chunks[1], state);

    // Controls hint
    let controls = if state.creating_dir.is_some() {
        Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Green)),
            Span::styled(" create  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ])
    } else if let Some(ref msg) = state.status_message {
        Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(Color::Green),
        ))
    } else {
        Line::from(vec![
            Span::styled(" \u{25b2}/\u{25bc}", Style::default().fg(Color::Yellow)),
            Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("\u{2192}/\u{2190}", Style::default().fg(Color::Yellow)),
            Span::styled(" expand  ", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::Yellow)),
            Span::styled(" new dir  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" exit", Style::default().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(Paragraph::new(controls), chunks[2]);
}

fn render_tree(f: &mut ratatui::Frame, area: Rect, state: &mut DirectoryPickerState) {
    let tree_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = tree_block.inner(area);
    f.render_widget(tree_block, area);

    let visible_height = inner.height as usize;
    state.navigator.set_visible_height(visible_height);

    let entries = state.navigator.entries();
    let cursor_idx = state.navigator.cursor_idx();
    let scroll = state.navigator.scroll_offset();

    let creating_at_cursor = state.creating_dir.is_some();
    let mut lines: Vec<Line> = Vec::new();

    for (idx, entry) in entries.iter().enumerate().skip(scroll).take(visible_height) {
        let is_cursor = idx == cursor_idx;

        if is_cursor && creating_at_cursor {
            // Render inline text input instead of the tree entry
            let creating = state.creating_dir.as_ref().unwrap();
            let indent = "  ".repeat(entry.depth);

            let text = creating.input.value().to_string();
            let cursor_pos = creating.input.cursor;
            let chars: Vec<char> = text.chars().collect();
            let before: String = chars[..cursor_pos].iter().collect();
            let at_cursor = chars
                .get(cursor_pos)
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".to_string());
            let after: String = if cursor_pos < chars.len() {
                chars[cursor_pos + 1..].iter().collect()
            } else {
                String::new()
            };

            lines.push(Line::from(vec![
                Span::raw(format!("{}  ", indent)),
                Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
                Span::styled(before, Style::default().fg(Color::White)),
                Span::styled(at_cursor, Style::default().fg(Color::Black).bg(Color::Cyan)),
                Span::styled(after, Style::default().fg(Color::White)),
            ]));
        } else {
            lines.push(render_picker_entry(entry, is_cursor));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_picker_entry(entry: &TreeEntry, is_cursor: bool) -> Line<'static> {
    let indent = "  ".repeat(entry.depth);

    let expand_indicator = if entry.is_directory() && !entry.is_synthetic {
        if entry.has_children {
            if entry.is_expanded {
                "\u{25bc} "
            } else {
                "\u{25b6} "
            }
        } else {
            "  "
        }
    } else {
        "  "
    };

    let name_style = if is_cursor {
        CURSOR_STYLE
    } else if entry.is_synthetic {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Blue)
    };

    Line::from(vec![
        Span::raw(indent),
        Span::styled(expand_indicator, Style::default().fg(Color::Yellow)),
        Span::styled(entry.name.clone(), name_style),
    ])
}

// ============================================================================
// Step 2: Create First Account
// ============================================================================

/// Focus state for the account creation form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountField {
    Username,
    Password,
    Confirm,
}

/// State for the account creation step.
struct CreateAccountState {
    username: TextInputState,
    password: TextInputState,
    confirm: TextInputState,
    focused: AccountField,
    error_message: Option<String>,
}

impl CreateAccountState {
    fn new() -> Self {
        let mut username = TextInputState::new();
        username.focused = true;
        Self {
            username,
            password: TextInputState::new(),
            confirm: TextInputState::new(),
            focused: AccountField::Username,
            error_message: None,
        }
    }

    fn focus_field(&mut self, field: AccountField) {
        self.username.focused = false;
        self.password.focused = false;
        self.confirm.focused = false;
        match field {
            AccountField::Username => self.username.focused = true,
            AccountField::Password => self.password.focused = true,
            AccountField::Confirm => self.confirm.focused = true,
        }
        self.focused = field;
    }

    fn focus_next(&mut self) {
        let next = match self.focused {
            AccountField::Username => AccountField::Password,
            AccountField::Password => AccountField::Confirm,
            AccountField::Confirm => AccountField::Username,
        };
        self.focus_field(next);
    }

    fn focus_prev(&mut self) {
        let prev = match self.focused {
            AccountField::Username => AccountField::Confirm,
            AccountField::Password => AccountField::Username,
            AccountField::Confirm => AccountField::Password,
        };
        self.focus_field(prev);
    }
}

/// Run the "Create Account" step of first-time setup.
///
/// Returns `(username, password_hash)` on success.
pub fn run_create_account<B: Backend>(
    terminal: &mut Terminal<B>,
) -> Result<(String, String)> {
    let mut state = CreateAccountState::new();

    loop {
        terminal.draw(|f| render_create_account(f, &state))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => {
                    state.focus_next();
                    state.error_message = None;
                }
                KeyCode::BackTab => {
                    state.focus_prev();
                    state.error_message = None;
                }
                KeyCode::Down => {
                    state.focus_next();
                    state.error_message = None;
                }
                KeyCode::Up => {
                    state.focus_prev();
                    state.error_message = None;
                }
                KeyCode::Enter => {
                    // Validate and submit
                    let username = state.username.value().trim().to_string();
                    let password = state.password.value().to_string();
                    let confirm = state.confirm.value().to_string();

                    if username.is_empty() {
                        state.error_message = Some("Username cannot be empty".to_string());
                        state.focus_field(AccountField::Username);
                    } else if password.is_empty() {
                        state.error_message = Some("Password cannot be empty".to_string());
                        state.focus_field(AccountField::Password);
                    } else if password != confirm {
                        state.error_message = Some("Passwords do not match".to_string());
                        state.confirm.clear();
                        state.focus_field(AccountField::Confirm);
                    } else {
                        // Return plaintext credentials — server hashes at storage time
                        return Ok((username, password));
                    }
                }
                KeyCode::Esc => {
                    return Err(anyhow::anyhow!("Setup cancelled by user"));
                }
                _ => {
                    let action = input::map_key(key);
                    match state.focused {
                        AccountField::Username => {
                            state.username.handle_input(&action);
                        }
                        AccountField::Password => {
                            state.password.handle_input(&action);
                        }
                        AccountField::Confirm => {
                            state.confirm.handle_input(&action);
                        }
                    }
                    state.error_message = None;
                }
            }
        }
    }
}

fn render_create_account(f: &mut ratatui::Frame, state: &CreateAccountState) {
    let area = f.area();

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" First-Time Setup \u{2014} Step 2 of 3 ")
        .title_alignment(Alignment::Center);

    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    let dialog_width = 50.min(inner.width.saturating_sub(4));
    let dialog_height = 16.min(inner.height.saturating_sub(4));

    let dialog_area = Rect {
        x: inner.x + (inner.width.saturating_sub(dialog_width)) / 2,
        y: inner.y + (inner.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(" Create Account ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let form_area = block.inner(dialog_area);
    f.render_widget(block, dialog_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Description
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Username label
            Constraint::Length(1), // Username input
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Password label
            Constraint::Length(1), // Password input
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Confirm label
            Constraint::Length(1), // Confirm input
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Error / controls
        ])
        .split(form_area);

    f.render_widget(
        Paragraph::new(" Create your administrator account.")
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    // Username
    let label_style = if state.focused == AccountField::Username {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(" Username:").style(label_style),
        chunks[2],
    );
    render_text_field(f, chunks[3], &state.username, false);

    // Password
    let label_style = if state.focused == AccountField::Password {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(" Password:").style(label_style),
        chunks[5],
    );
    render_text_field(f, chunks[6], &state.password, true);

    // Confirm
    let label_style = if state.focused == AccountField::Confirm {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(" Confirm:").style(label_style),
        chunks[8],
    );
    render_text_field(f, chunks[9], &state.confirm, true);

    // Error or controls
    if let Some(ref err) = state.error_message {
        f.render_widget(
            Paragraph::new(format!(" {}", err)).style(Style::default().fg(Color::Red)),
            chunks[11],
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Tab", Style::default().fg(Color::Yellow)),
                Span::styled(" next  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Enter", Style::default().fg(Color::Green)),
                Span::styled(" create  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Esc", Style::default().fg(Color::DarkGray)),
                Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
            ])),
            chunks[11],
        );
    }
}

/// Render a text input field, optionally masked for passwords.
fn render_text_field(
    f: &mut ratatui::Frame,
    area: Rect,
    input: &TextInputState,
    masked: bool,
) {
    let value = input.value().to_string();
    let display: String = if masked {
        "*".repeat(value.chars().count())
    } else {
        value.clone()
    };

    if input.focused {
        let cursor_pos = input.cursor;
        let chars: Vec<char> = display.chars().collect();
        let before: String = chars[..cursor_pos.min(chars.len())].iter().collect();
        let at_cursor = chars
            .get(cursor_pos)
            .map(|c| c.to_string())
            .unwrap_or_else(|| " ".to_string());
        let after: String = if cursor_pos < chars.len() {
            chars[cursor_pos + 1..].iter().collect()
        } else {
            String::new()
        };

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(before, Style::default().fg(Color::White)),
                Span::styled(
                    at_cursor,
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ),
                Span::styled(after, Style::default().fg(Color::White)),
            ])),
            area,
        );
    } else {
        f.render_widget(
            Paragraph::new(format!(" {}", display)).style(Style::default().fg(Color::White)),
            area,
        );
    }
}
