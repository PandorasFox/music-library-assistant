//! TUI Login Screen
//!
//! Shown when the system has users (NeedsAuth) and no active session.
//! Blocks until valid credentials are provided.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;

use crate::auth::SessionToken;
use crate::ui::input;
use crate::ui::widgets::TextInputState;
use crate::witch::WitchHandle;

/// Focus state for the login form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginField {
    Username,
    Password,
}

/// State for the login screen.
struct LoginState {
    username: TextInputState,
    password: TextInputState,
    focused: LoginField,
    error_message: Option<String>,
}

impl LoginState {
    fn new() -> Self {
        let mut username = TextInputState::new();
        username.focused = true;
        Self {
            username,
            password: TextInputState::new(),
            focused: LoginField::Username,
            error_message: None,
        }
    }

    fn focus_field(&mut self, field: LoginField) {
        self.username.focused = false;
        self.password.focused = false;
        match field {
            LoginField::Username => self.username.focused = true,
            LoginField::Password => self.password.focused = true,
        }
        self.focused = field;
    }

    fn focus_next(&mut self) {
        let next = match self.focused {
            LoginField::Username => LoginField::Password,
            LoginField::Password => LoginField::Username,
        };
        self.focus_field(next);
    }
}

/// Run the login screen, blocking until valid credentials are provided.
///
/// Returns the session token on success.
pub fn run_login_screen<B: Backend>(
    terminal: &mut Terminal<B>,
    witch: &mut WitchHandle,
) -> Result<SessionToken> {
    let mut state = LoginState::new();

    loop {
        terminal.draw(|f| render_login(f, &state))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab | KeyCode::Down => {
                    state.focus_next();
                    state.error_message = None;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.focus_next(); // Only 2 fields, next == prev
                    state.error_message = None;
                }
                KeyCode::Enter => {
                    let username = state.username.value().trim().to_string();
                    let password = state.password.value().to_string();

                    if username.is_empty() {
                        state.error_message = Some("Username required".to_string());
                        state.focus_field(LoginField::Username);
                        continue;
                    }

                    match witch.login(&username, &password) {
                        Ok(token) => return Ok(token),
                        Err(e) => {
                            state.error_message = Some(e);
                            state.password.clear();
                            state.focus_field(LoginField::Password);
                        }
                    }
                }
                KeyCode::Esc => {
                    return Err(anyhow::anyhow!("Login cancelled by user"));
                }
                _ => {
                    let action = input::map_key(key);
                    match state.focused {
                        LoginField::Username => {
                            state.username.handle_input(&action);
                        }
                        LoginField::Password => {
                            state.password.handle_input(&action);
                        }
                    }
                    state.error_message = None;
                }
            }
        }
    }
}

fn render_login(f: &mut ratatui::Frame, state: &LoginState) {
    let area = f.area();

    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 14.min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(format!(" {} ", crate::MM_TITLE))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let form_area = block.inner(dialog_area);
    f.render_widget(block, dialog_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Title
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Username label
            Constraint::Length(1), // Username input
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Password label
            Constraint::Length(1), // Password input
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Error / controls
        ])
        .split(form_area);

    f.render_widget(
        Paragraph::new(" Login")
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
        chunks[0],
    );

    // Username
    let label_style = if state.focused == LoginField::Username {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(" Username:").style(label_style),
        chunks[2],
    );
    render_login_field(f, chunks[3], &state.username, false);

    // Password
    let label_style = if state.focused == LoginField::Password {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(" Password:").style(label_style),
        chunks[5],
    );
    render_login_field(f, chunks[6], &state.password, true);

    // Error or controls
    if let Some(ref err) = state.error_message {
        f.render_widget(
            Paragraph::new(format!(" {}", err)).style(Style::default().fg(Color::Red)),
            chunks[8],
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Tab", Style::default().fg(Color::Yellow)),
                Span::styled(" next  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Enter", Style::default().fg(Color::Green)),
                Span::styled(" login  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Esc", Style::default().fg(Color::DarkGray)),
                Span::styled(" exit", Style::default().fg(Color::DarkGray)),
            ])),
            chunks[8],
        );
    }
}

/// Render a text input field, optionally masked for passwords.
fn render_login_field(
    f: &mut ratatui::Frame,
    area: Rect,
    input: &TextInputState,
    masked: bool,
) {
    let value = input.value().to_string();
    let display: String = if masked {
        "*".repeat(value.chars().count())
    } else {
        value
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
