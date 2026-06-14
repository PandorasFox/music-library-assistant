//! First-Time Setup UI Components
//!
//! Provides the interactive UI elements for first-time setup, called from
//! `run_tui()` when the Witch is in `AwaitingSetup` state:
//!
//! - `run_first_time_setup()`: Unified form collecting storage root + admin credentials.
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
// First-Time Setup Form
// ============================================================================

/// Output from first-time setup: storage root path and admin credentials.
pub struct FirstTimeSetupResult {
    pub storage_root: PathBuf,
    pub username: String,
    pub password: String,
}

/// State for the unified first-time setup form.
///
/// Currently uses text input for storage root. The `storage_root` field can be
/// replaced with a directory picker component in the future while keeping the
/// same external interface.
pub struct FirstTimeSetupState {
    /// Storage root path input. TODO: Replace with DirectoryPicker for better UX.
    pub storage_root: TextInputState,
    pub username: TextInputState,
    pub password: TextInputState,
    pub confirm_password: TextInputState,
    /// Which field has focus (0=storage_root, 1=username, 2=password, 3=confirm)
    pub focus: usize,
    /// Error message to display
    pub error: Option<String>,
}

impl FirstTimeSetupState {
    pub fn new(suggested_root: Option<PathBuf>) -> Self {
        let mut storage_root = TextInputState::new();
        if let Some(path) = suggested_root {
            storage_root.set_value(path.to_string_lossy().into_owned());
        }
        storage_root.focused = true;

        Self {
            storage_root,
            username: TextInputState::new(),
            password: TextInputState::new(),
            confirm_password: TextInputState::new(),
            focus: 0,
            error: None,
        }
    }

    fn field_count(&self) -> usize {
        4
    }

    fn focus_field(&mut self, index: usize) {
        // Unfocus all
        self.storage_root.focused = false;
        self.username.focused = false;
        self.password.focused = false;
        self.confirm_password.focused = false;
        // Focus the target
        self.focus = index % self.field_count();
        match self.focus {
            0 => self.storage_root.focused = true,
            1 => self.username.focused = true,
            2 => self.password.focused = true,
            3 => self.confirm_password.focused = true,
            _ => {}
        }
    }

    fn focus_next(&mut self) {
        self.focus_field(self.focus + 1);
    }

    fn focus_prev(&mut self) {
        self.focus_field(if self.focus == 0 { self.field_count() - 1 } else { self.focus - 1 });
    }

    fn current_input(&mut self) -> &mut TextInputState {
        match self.focus {
            0 => &mut self.storage_root,
            1 => &mut self.username,
            2 => &mut self.password,
            _ => &mut self.confirm_password,
        }
    }

    fn validate(&self) -> Result<FirstTimeSetupResult, String> {
        let storage_root_str = self.storage_root.value();
        if storage_root_str.is_empty() {
            return Err("Storage root cannot be empty".into());
        }
        let storage_root = PathBuf::from(storage_root_str);
        if !storage_root.is_dir() {
            return Err(format!("'{}' is not a directory", storage_root_str));
        }

        let username = self.username.value().to_string();
        if username.is_empty() {
            return Err("Username cannot be empty".into());
        }

        let password = self.password.value().to_string();
        if password.is_empty() {
            return Err("Password cannot be empty".into());
        }

        if password != self.confirm_password.value() {
            return Err("Passwords do not match".into());
        }

        Ok(FirstTimeSetupResult {
            storage_root,
            username,
            password,
        })
    }
}

/// Run the unified first-time setup form.
///
/// Collects storage root path and admin credentials in a single form.
/// Returns the validated setup result or an error if cancelled.
pub fn run_first_time_setup<B: Backend>(
    terminal: &mut Terminal<B>,
    suggested_root: Option<PathBuf>,
) -> Result<FirstTimeSetupResult> {
    let mut state = FirstTimeSetupState::new(suggested_root);

    loop {
        terminal.draw(|f| render_first_time_setup(f, &state))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                let action = input::map_key(key);

                match action {
                    input::InputAction::Cancel => {
                        anyhow::bail!("Setup cancelled");
                    }
                    input::InputAction::CycleNext | input::InputAction::NavDown => {
                        state.focus_next();
                    }
                    input::InputAction::NavUp => {
                        state.focus_prev();
                    }
                    input::InputAction::Confirm => {
                        match state.validate() {
                            Ok(result) => return Ok(result),
                            Err(msg) => state.error = Some(msg),
                        }
                    }
                    other => {
                        state.error = None;
                        state.current_input().handle_input(&other);
                    }
                }
            }
        }
    }
}

fn render_first_time_setup(f: &mut ratatui::Frame, state: &FirstTimeSetupState) {
    let area = f.area();

    // Center a 60x16 box
    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let popup_height = 16u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(" First-Time Setup ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // storage root (needs more space for path)
            Constraint::Length(2), // username
            Constraint::Length(2), // password
            Constraint::Length(2), // confirm
            Constraint::Length(2), // error/hint
            Constraint::Min(0),
        ])
        .split(inner);

    // Storage root
    render_field(f, chunks[0], "Storage Root:", state.storage_root.value(), state.focus == 0, false);

    // Username
    render_field(f, chunks[1], "Username:", state.username.value(), state.focus == 1, false);

    // Password
    render_field(f, chunks[2], "Password:", state.password.value(), state.focus == 2, true);

    // Confirm password
    render_field(f, chunks[3], "Confirm:", state.confirm_password.value(), state.focus == 3, true);

    // Error or hint
    if let Some(ref err) = state.error {
        f.render_widget(
            Paragraph::new(Span::styled(err, Style::default().fg(Color::Red))),
            chunks[4],
        );
    } else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "Tab/Arrows: navigate  Enter: submit  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            )),
            chunks[4],
        );
    }
}

fn render_field(f: &mut ratatui::Frame, area: Rect, label: &str, value: &str, focused: bool, masked: bool) {
    let label_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let display_value = if masked {
        "*".repeat(value.len())
    } else {
        value.to_string()
    };

    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(label, label_style)),
            Line::from(Span::styled(display_value, Style::default().fg(Color::White))),
        ]),
        area,
    );
}
