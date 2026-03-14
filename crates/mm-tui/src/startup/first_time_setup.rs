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
use crossterm::event::{self, Event};
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::path::PathBuf;

use crate::input;
use crate::widgets::TextInputState;

// ============================================================================
// Directory Picker
// ============================================================================

/// Run the directory picker, returning the selected path.
///
/// Called from `run_tui()` during first-time setup. The caller (TUI) owns the terminal.
/// If `suggested_root` is provided (e.g. from MM_ROOT env var), the picker
/// navigates there initially.
pub fn run_directory_picker<B: Backend>(
    _terminal: &mut Terminal<B>,
    _suggested_root: Option<PathBuf>,
) -> Result<PathBuf> {
    // TODO: reconnect when first-time setup directory picker is migrated to
    // protocol-driven DirectoryBrowser (needs pre-auth filesystem listing query)
    todo!("first-time setup directory picker needs protocol-driven migration")
}

// ============================================================================
// Account Creation
// ============================================================================

/// State for the create-account step.
pub struct CreateAccountState {
    pub username: TextInputState,
    pub password: TextInputState,
    pub confirm_password: TextInputState,
    /// Which field has focus (0=username, 1=password, 2=confirm)
    pub focus: usize,
    /// Error message to display
    pub error: Option<String>,
}

impl CreateAccountState {
    pub fn new() -> Self {
        let mut username = TextInputState::new();
        username.focused = true;
        Self {
            username,
            password: TextInputState::new(),
            confirm_password: TextInputState::new(),
            focus: 0,
            error: None,
        }
    }
}

/// Run the create-account form, returning (username, password).
///
/// Called from `run_tui()` during first-time setup when `has_any_users` is false.
pub fn run_create_account<B: Backend>(
    terminal: &mut Terminal<B>,
) -> Result<(String, String)> {
    let mut state = CreateAccountState::new();

    loop {
        terminal.draw(|f| render_create_account(f, &state))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                let action = input::map_key(key);

                match action {
                    input::InputAction::Cancel => {
                        anyhow::bail!("Setup cancelled");
                    }
                    input::InputAction::CycleNext | input::InputAction::NavDown => {
                        // Unfocus current, focus next
                        match state.focus {
                            0 => state.username.focused = false,
                            1 => state.password.focused = false,
                            2 => state.confirm_password.focused = false,
                            _ => {}
                        }
                        state.focus = (state.focus + 1) % 3;
                        match state.focus {
                            0 => state.username.focused = true,
                            1 => state.password.focused = true,
                            2 => state.confirm_password.focused = true,
                            _ => {}
                        }
                    }
                    input::InputAction::NavUp => {
                        match state.focus {
                            0 => state.username.focused = false,
                            1 => state.password.focused = false,
                            2 => state.confirm_password.focused = false,
                            _ => {}
                        }
                        state.focus = if state.focus == 0 { 2 } else { state.focus - 1 };
                        match state.focus {
                            0 => state.username.focused = true,
                            1 => state.password.focused = true,
                            2 => state.confirm_password.focused = true,
                            _ => {}
                        }
                    }
                    input::InputAction::Confirm => {
                        let username = state.username.value().to_string();
                        let password = state.password.value().to_string();
                        let confirm = state.confirm_password.value().to_string();

                        if username.is_empty() {
                            state.error = Some("Username cannot be empty".to_string());
                        } else if password.is_empty() {
                            state.error = Some("Password cannot be empty".to_string());
                        } else if password != confirm {
                            state.error = Some("Passwords do not match".to_string());
                        } else {
                            return Ok((username, password));
                        }
                    }
                    other => {
                        state.error = None;
                        match state.focus {
                            0 => { state.username.handle_input(&other); }
                            1 => { state.password.handle_input(&other); }
                            2 => { state.confirm_password.handle_input(&other); }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

fn render_create_account(f: &mut ratatui::Frame, state: &CreateAccountState) {
    let area = f.area();

    // Center a 50x12 box
    let popup_width = 50u16.min(area.width.saturating_sub(4));
    let popup_height = 12u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(" Create Admin Account ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // username
            Constraint::Length(2), // password
            Constraint::Length(2), // confirm
            Constraint::Length(1), // error/hint
            Constraint::Min(0),
        ])
        .split(inner);

    // Username
    let label_style = if state.focus == 0 {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("Username:", label_style)),
            Line::from(Span::styled(
                state.username.value(),
                Style::default().fg(Color::White),
            )),
        ]),
        chunks[0],
    );

    // Password
    let label_style = if state.focus == 1 {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let masked: String = "*".repeat(state.password.value().len());
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("Password:", label_style)),
            Line::from(Span::styled(masked, Style::default().fg(Color::White))),
        ]),
        chunks[1],
    );

    // Confirm
    let label_style = if state.focus == 2 {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let masked: String = "*".repeat(state.confirm_password.value().len());
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("Confirm:", label_style)),
            Line::from(Span::styled(masked, Style::default().fg(Color::White))),
        ]),
        chunks[2],
    );

    // Error or hint
    if let Some(ref err) = state.error {
        f.render_widget(
            Paragraph::new(Span::styled(err, Style::default().fg(Color::Red))),
            chunks[3],
        );
    } else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "Tab: next field  Enter: submit  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[3],
        );
    }
}
