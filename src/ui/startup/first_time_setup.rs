//! First-Time Setup Wizard
//!
//! Two code paths:
//!
//! 1. **Full setup** (`run_first_time_setup`): No config exists at all. Runs an
//!    interactive directory picker to select the archive root, creates config.kdl,
//!    subdirectories, and DB. Manages its own terminal lifecycle.
//!
//! 2. **DB-only setup** (`handle_first_time_setup`): Config exists but DB was deleted.
//!    Simple confirmation dialog that recreates the DB and subdirectories. Runs within
//!    an already-initialized terminal (called from `run_menu`).

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::io;
use std::path::{Path, PathBuf};

use crate::ui::tree_browser::{EntryFilter, TreeEntry, TreeNavigator};

/// Proof that code is executing in the first-time setup path.
/// Constructor is private to this module; type is pub(crate) so db/ can require it.
pub(crate) struct FirstTimeSetupToken(());

use crate::ui::input;
use crate::ui::widgets::selection_styles::CURSOR_STYLE;
use crate::ui::widgets::TextInputState;

// ============================================================================
// Full First-Time Setup (no config exists)
// ============================================================================

/// Run the complete first-time setup wizard with its own terminal.
///
/// Called from main() when no config.kdl exists. Guides the user through:
/// 1. Selecting an archive root directory (with option to create new dirs)
/// 2. Creating subdirectories + database
pub fn run_first_time_setup() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_setup_flow(&mut terminal);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_setup_flow<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    // Step 1: Directory picker
    let selected_root = run_directory_picker(terminal)?;

    // Write initial config
    crate::config::write_initial_config(&selected_root)?;

    // Create subdirectories
    std::fs::create_dir_all(selected_root.join("corpus"))?;
    std::fs::create_dir_all(selected_root.join("libraries"))?;
    std::fs::create_dir_all(selected_root.join("stash"))?;
    std::fs::create_dir_all(selected_root.join("inbox"))?;

    // Create database
    let db_path = crate::config::get_db_path()?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let token = FirstTimeSetupToken(());
    let db = crate::db::create_database(&db_path, &token)?;
    drop(db);

    // Step 2: Show completion + hint
    run_completion_screen(terminal, &selected_root, &db_path)?;

    Ok(())
}

// ============================================================================
// Step 1: Directory Picker
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
fn run_directory_picker<B: Backend>(terminal: &mut Terminal<B>) -> Result<PathBuf> {
    let mut state = DirectoryPickerState::new();

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
        .title(" First-Time Setup \u{2014} Step 1 of 2 ")
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
// Step 2: Completion Screen
// ============================================================================

fn run_completion_screen<B: Backend>(
    terminal: &mut Terminal<B>,
    root: &Path,
    db_path: &Path,
) -> Result<()> {
    let root_display = root.display().to_string();
    let db_display = db_path.display().to_string();

    loop {
        terminal.draw(|f| {
            render_completion_screen(f, &root_display, &db_display);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => break,
                _ => {}
            }
        }
    }

    Ok(())
}

fn render_completion_screen(f: &mut ratatui::Frame, root_display: &str, db_display: &str) {
    let area = f.area();

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" First-Time Setup \u{2014} Step 2 of 2 ")
        .title_alignment(Alignment::Center);

    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" Archive root: ", Style::default().fg(Color::DarkGray)),
            Span::styled(root_display, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Directory structure created:",
            Style::default().fg(Color::White),
        )),
        Line::from(vec![
            Span::styled("   \u{2713} ", Style::default().fg(Color::Green)),
            Span::styled("corpus/", Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("   \u{2713} ", Style::default().fg(Color::Green)),
            Span::styled("libraries/", Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("   \u{2713} ", Style::default().fg(Color::Green)),
            Span::styled("stash/", Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("   \u{2713} ", Style::default().fg(Color::Green)),
            Span::styled("inbox/", Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Database created:",
            Style::default().fg(Color::White),
        )),
        Line::from(vec![
            Span::styled("   \u{2713} ", Style::default().fg(Color::Green)),
            Span::styled(db_display, Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " If you have an existing music library, now is a good time to",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            " copy or move it into the corpus/ directory. MM will discover",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            " and index everything on its first scan.",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" [", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled("] Continue to MM", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

// ============================================================================
// DB-Only First-Time Setup (config exists, DB was deleted)
// ============================================================================

/// Handle first-time DB setup when config exists but no database file exists.
///
/// Shows a dialog prompting the user to create a new database, then
/// initializes it with the latest schema version (no migrations needed).
pub fn handle_first_time_setup<B: Backend>(
    terminal: &mut Terminal<B>,
    db_path: &Path,
) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db_display = db_path.to_string_lossy();

    // Render first-time setup dialog
    loop {
        terminal.draw(|f| {
            let area = f.area();

            let dialog_width = 65.min(area.width.saturating_sub(4));
            let dialog_height = 14.min(area.height.saturating_sub(4));

            let dialog_area = Rect {
                x: (area.width.saturating_sub(dialog_width)) / 2,
                y: (area.height.saturating_sub(dialog_height)) / 2,
                width: dialog_width,
                height: dialog_height,
            };

            f.render_widget(Clear, dialog_area);

            let lines = vec![
                Line::from(""),
                Line::from("Welcome to MM!").style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::from(""),
                Line::from("No database found. MM will create a new one at:")
                    .style(Style::default().fg(Color::White)),
                Line::from(""),
                Line::from(format!("  {}", db_display)).style(Style::default().fg(Color::Yellow)),
                Line::from(""),
                Line::from("After setup, your corpus will be scanned.")
                    .style(Style::default().fg(Color::DarkGray)),
                Line::from(""),
                Line::from("[Enter] Create Database    [Esc] Exit")
                    .style(Style::default().fg(Color::Cyan)),
            ];

            let paragraph = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" First-Time Setup ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .alignment(Alignment::Center);

            f.render_widget(paragraph, dialog_area);
        })?;

        // Wait for user input
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    // User confirmed first-time setup. No ConfirmationGesture needed here:
                    // database creation is infrastructure setup, not a corpus mutation.
                    // ConfirmationGesture is for mutations that alter indexed corpus data.
                    break;
                }
                KeyCode::Esc => {
                    return Err(anyhow::anyhow!("Setup cancelled by user"));
                }
                _ => {}
            }
        }
    }

    // Show "creating database" status
    terminal.draw(|f| {
        let area = f.area();
        let dialog_width = 50.min(area.width.saturating_sub(4));
        let dialog_height = 7;

        let dialog_area = Rect {
            x: (area.width.saturating_sub(dialog_width)) / 2,
            y: (area.height.saturating_sub(dialog_height)) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        f.render_widget(Clear, dialog_area);

        let paragraph = Paragraph::new(vec![
            Line::from(""),
            Line::from("Creating database...").style(Style::default().fg(Color::Cyan)),
            Line::from(""),
        ])
        .block(
            Block::default()
                .title(" Setup ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center);

        f.render_widget(paragraph, dialog_area);
    })?;

    // Create archive subdirectories
    let config = crate::config::load_config()?;
    std::fs::create_dir_all(config.corpus_dir())?;
    std::fs::create_dir_all(config.libraries_dir())?;
    std::fs::create_dir_all(config.stash_dir())?;
    std::fs::create_dir_all(config.inbox_dir())?;

    // Create database (initialize_schema stores the schema fingerprint)
    let token = FirstTimeSetupToken(());
    let db = crate::db::create_database(db_path, &token)?;
    drop(db);

    // Show completion message
    terminal.draw(|f| {
        let area = f.area();
        let dialog_width = 50.min(area.width.saturating_sub(4));
        let dialog_height = 7;

        let dialog_area = Rect {
            x: (area.width.saturating_sub(dialog_width)) / 2,
            y: (area.height.saturating_sub(dialog_height)) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        f.render_widget(Clear, dialog_area);

        let paragraph = Paragraph::new(vec![
            Line::from(""),
            Line::from("Database created successfully!").style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(""),
            Line::from("Starting initial corpus scan...")
                .style(Style::default().fg(Color::DarkGray)),
        ])
        .block(
            Block::default()
                .title(" Setup Complete ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        )
        .alignment(Alignment::Center);

        f.render_widget(paragraph, dialog_area);
    })?;

    std::thread::sleep(std::time::Duration::from_millis(800));
    Ok(())
}
