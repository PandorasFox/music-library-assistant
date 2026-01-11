//! Reusable TUI Picker Component
//!
//! General-purpose list picker for selecting from 2-10 options
//! using arrow key navigation

use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    backend::Backend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};

/// Generic picker for selecting from multiple options
pub struct Picker {
    options: Vec<PickerOption>,
    state: ListState,
    title: String,
    description: Option<String>,
}

/// An option in the picker list
pub struct PickerOption {
    pub label: String,
    pub description: Option<String>,
    pub value: String, // Internal value (e.g., directory name)
}

/// Result of running the picker
pub enum PickerResult {
    Selected(String), // User selected an option (returns value)
    Cancelled,        // User pressed Esc
}

impl Picker {
    /// Create a new picker with a title and list of options
    pub fn new(title: String, options: Vec<PickerOption>) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));

        Self {
            options,
            state,
            title,
            description: None,
        }
    }

    /// Add an optional description below the title
    pub fn with_description(mut self, desc: String) -> Self {
        self.description = Some(desc);
        self
    }

    /// Run the picker and return user selection
    pub fn run<B: Backend>(
        mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<PickerResult, Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|f| self.render(f))?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Up => self.previous(),
                    KeyCode::Down => self.next(),
                    KeyCode::Enter => {
                        let selected_idx = self.state.selected().unwrap_or(0);
                        return Ok(PickerResult::Selected(
                            self.options[selected_idx].value.clone()
                        ));
                    }
                    KeyCode::Esc => return Ok(PickerResult::Cancelled),
                    _ => {}
                }
            }
        }
    }

    fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.options.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.options.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn render<B: Backend>(&mut self, f: &mut Frame<B>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([
                Constraint::Length(3),  // Title
                Constraint::Length(if self.description.is_some() { 2 } else { 0 }),  // Description
                Constraint::Min(10),    // List
                Constraint::Length(2),  // Instructions
            ])
            .split(f.size());

        // Title
        let title_para = Paragraph::new(self.title.clone())
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::BOTTOM));
        f.render_widget(title_para, chunks[0]);

        // Description (if present)
        if let Some(desc) = &self.description {
            let desc_para = Paragraph::new(desc.clone())
                .style(Style::default().fg(Color::Gray));
            f.render_widget(desc_para, chunks[1]);
        }

        // List of options
        let items: Vec<ListItem> = self.options.iter().enumerate()
            .map(|(i, opt)| {
                let number = format!("[{}] ", i + 1);
                let label = &opt.label;

                let line = Line::from(vec![
                    Span::styled(number, Style::default().fg(Color::Yellow)),
                    Span::raw(label),
                ]);

                ListItem::new(line)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Options"))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            )
            .highlight_symbol(">> ");

        f.render_stateful_widget(list, chunks[2], &mut self.state);

        // Instructions
        let instructions = Paragraph::new("↑/↓: Navigate  Enter: Select  Esc: Cancel")
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(instructions, chunks[3]);
    }
}
