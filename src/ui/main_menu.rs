//! Multi-pane Main Menu Navigation
//!
//! Provides a three-column navigation system:
//! - Left pane: Categories (Build Indices, Insight, Corpus Ops, etc.)
//! - Middle pane: Commands within selected category
//! - Right pane: Info/description for highlighted command (TODO placeholders)

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::config::Config;

// ============================================================================
// Types
// ============================================================================

/// Which pane currently has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Category,
    Command,
}

/// Main menu navigation state
#[derive(Debug)]
pub struct MainMenuState {
    /// Currently highlighted category index
    pub category_index: usize,
    /// Currently highlighted command index within category
    pub command_index: usize,
    /// Which pane has focus
    pub focus: PaneFocus,
    /// List state for category pane
    pub category_list_state: ListState,
    /// List state for command pane
    pub command_list_state: ListState,
    /// Built menu structure
    pub categories: Vec<Category>,
}

/// Category definition with commands
#[derive(Debug, Clone)]
pub struct Category {
    pub name: String,
    pub commands: Vec<Command>,
}

/// Individual command within a category
#[derive(Debug, Clone)]
pub struct Command {
    pub label: String,
    pub action: CommandAction,
    pub description: String, // TODO: Fill in descriptions later
}

/// What happens when a command is executed
#[derive(Debug, Clone)]
pub enum CommandAction {
    /// Execute immediately in background (scan, report generation)
    Background(BackgroundTask),
    /// Transition to another UI mode
    Transition(TransitionTarget),
    /// Show message and stay in menu
    Message(String),
    /// Quit the application
    Quit,
}

/// Background tasks that can run while UI stays responsive
#[derive(Debug, Clone)]
pub enum BackgroundTask {
    /// Scan the main corpus
    ScanCorpus { re_fingerprint: bool },
    /// Scan the legacy library (if configured)
    ScanLegacy,
    GenerateReport { report_type: ReportType },
    Deploy { dry_run: bool },
}

/// Target UI modes for transitions
#[derive(Debug, Clone, Copy)]
pub enum TransitionTarget {
    TagEditor,
    DecisionFlow,
    PendingChangesView,
}

/// Report types (mirrored from legacy code)
#[derive(Debug, Clone, Copy)]
pub enum ReportType {
    GenerateAll,
    Legacy,
    Deployment,
    Quality,
    Duplicates,
}

/// Result of handling a key press
#[derive(Debug)]
pub enum MenuAction {
    /// No action, continue in menu
    None,
    /// Execute a command action
    Execute(CommandAction),
    /// Quit the application
    Quit,
}

// ============================================================================
// Implementation
// ============================================================================

impl MainMenuState {
    /// Create a new main menu state with categories built from config
    pub fn new(config: &Config) -> Self {
        let categories = build_menu_categories(config);

        let mut category_list_state = ListState::default();
        category_list_state.select(Some(0));

        let mut command_list_state = ListState::default();
        command_list_state.select(Some(0));

        Self {
            category_index: 0,
            command_index: 0,
            focus: PaneFocus::Category,
            category_list_state,
            command_list_state,
            categories,
        }
    }

    /// Handle a key event and return the resulting action
    pub fn handle_key(&mut self, key: KeyEvent) -> MenuAction {
        match key.code {
            KeyCode::Up => {
                self.move_up();
                MenuAction::None
            }
            KeyCode::Down => {
                self.move_down();
                MenuAction::None
            }
            KeyCode::Left => {
                self.move_left();
                MenuAction::None
            }
            KeyCode::Right => {
                self.move_right();
                MenuAction::None
            }
            KeyCode::Enter => self.execute_selection(),
            KeyCode::Esc => {
                match self.focus {
                    PaneFocus::Command => {
                        // Go back to category pane
                        self.focus = PaneFocus::Category;
                        MenuAction::None
                    }
                    PaneFocus::Category => {
                        // ESC from category = quit
                        MenuAction::Quit
                    }
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                if self.focus == PaneFocus::Category {
                    MenuAction::Quit
                } else {
                    MenuAction::None
                }
            }
            _ => MenuAction::None,
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            PaneFocus::Category => {
                if self.category_index > 0 {
                    self.category_index -= 1;
                    self.category_list_state.select(Some(self.category_index));
                    // Reset command index when changing categories
                    self.command_index = 0;
                    self.command_list_state.select(Some(0));
                }
            }
            PaneFocus::Command => {
                if self.command_index > 0 {
                    self.command_index -= 1;
                    self.command_list_state.select(Some(self.command_index));
                }
            }
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            PaneFocus::Category => {
                if self.category_index < self.categories.len().saturating_sub(1) {
                    self.category_index += 1;
                    self.category_list_state.select(Some(self.category_index));
                    // Reset command index when changing categories
                    self.command_index = 0;
                    self.command_list_state.select(Some(0));
                }
            }
            PaneFocus::Command => {
                let max_commands = self.current_category().map(|c| c.commands.len()).unwrap_or(0);
                if self.command_index < max_commands.saturating_sub(1) {
                    self.command_index += 1;
                    self.command_list_state.select(Some(self.command_index));
                }
            }
        }
    }

    fn move_left(&mut self) {
        if self.focus == PaneFocus::Command {
            self.focus = PaneFocus::Category;
        }
    }

    fn move_right(&mut self) {
        if self.focus == PaneFocus::Category {
            self.focus = PaneFocus::Command;
            // Ensure command is selected
            self.command_list_state.select(Some(self.command_index));
        }
    }

    fn execute_selection(&mut self) -> MenuAction {
        match self.focus {
            PaneFocus::Category => {
                // Enter from category = move to commands
                self.focus = PaneFocus::Command;
                self.command_list_state.select(Some(self.command_index));
                MenuAction::None
            }
            PaneFocus::Command => {
                // Enter from command = execute it
                if let Some(category) = self.current_category() {
                    if let Some(command) = category.commands.get(self.command_index) {
                        return MenuAction::Execute(command.action.clone());
                    }
                }
                MenuAction::None
            }
        }
    }

    fn current_category(&self) -> Option<&Category> {
        self.categories.get(self.category_index)
    }

    fn current_command(&self) -> Option<&Command> {
        self.current_category()
            .and_then(|c| c.commands.get(self.command_index))
    }

    /// Render the multi-pane main menu
    pub fn render(&mut self, f: &mut Frame, area: Rect, _config: &Option<Config>) {
        // Three-pane horizontal layout
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25), // Categories
                Constraint::Percentage(35), // Commands
                Constraint::Percentage(40), // Info
            ])
            .split(area);

        self.render_categories(f, chunks[0]);
        self.render_commands(f, chunks[1]);
        self.render_info(f, chunks[2]);
    }

    fn render_categories(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .categories
            .iter()
            .map(|cat| ListItem::new(Line::from(cat.name.as_str())))
            .collect();

        let highlight_style = if self.focus == PaneFocus::Category {
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Categories"))
            .highlight_style(highlight_style)
            .highlight_symbol(if self.focus == PaneFocus::Category {
                ">> "
            } else {
                "   "
            });

        f.render_stateful_widget(list, area, &mut self.category_list_state);
    }

    fn render_commands(&mut self, f: &mut Frame, area: Rect) {
        // Collect data first to avoid borrow conflicts
        let (items, title): (Vec<ListItem>, String) = if let Some(cat) = self.categories.get(self.category_index) {
            let items = cat
                .commands
                .iter()
                .map(|cmd| ListItem::new(Line::from(cmd.label.as_str())))
                .collect();
            (items, cat.name.clone())
        } else {
            (Vec::new(), "Commands".to_string())
        };

        let list = if self.focus == PaneFocus::Command {
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(title))
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol(">> ")
        } else {
            // When unfocused, no highlight styling at all
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(title))
        };

        f.render_stateful_widget(list, area, &mut self.command_list_state);
    }

    fn render_info(&self, f: &mut Frame, area: Rect) {
        let info_lines = self.get_info_content();

        let paragraph = Paragraph::new(info_lines.join("\n"))
            .block(Block::default().borders(Borders::ALL).title("Info"))
            .style(Style::default().fg(Color::Gray));

        f.render_widget(paragraph, area);
    }

    fn get_info_content(&self) -> Vec<String> {
        let mut lines = Vec::new();

        if let Some(command) = self.current_command() {
            lines.push(command.description.clone());
            lines.push(String::new());

            // Add context-specific info
            match &command.action {
                CommandAction::Background(BackgroundTask::ScanCorpus { re_fingerprint }) => {
                    if *re_fingerprint {
                        lines.push("Ignores scan cache - rescans all files.".to_string());
                        lines.push("Use when fingerprints need recomputation.".to_string());
                    } else {
                        lines.push("Uses cached timestamps for incremental scan.".to_string());
                        lines.push("Only processes new/modified files.".to_string());
                    }
                }
                CommandAction::Background(BackgroundTask::ScanLegacy) => {
                    lines.push("Scans legacy library for migration analysis.".to_string());
                }
                CommandAction::Background(BackgroundTask::GenerateReport { .. }) => {
                    lines.push("(Latest report results will appear here)".to_string());
                }
                CommandAction::Transition(target) => {
                    let desc = match target {
                        TransitionTarget::TagEditor => "Opens interactive tag editor",
                        TransitionTarget::DecisionFlow => "Opens decision-making workflow",
                        TransitionTarget::PendingChangesView => "Shows queued changes",
                    };
                    lines.push(desc.to_string());
                }
                _ => {}
            }
        } else {
            lines.push("Select a command to see details.".to_string());
        }

        lines
    }
}

// ============================================================================
// Menu Building
// ============================================================================

fn build_menu_categories(config: &Config) -> Vec<Category> {
    vec![
        build_build_indices_category(config),
        build_insight_category(),
        build_corpus_ops_category(),
        build_deployment_category(),
        build_intake_category(),
        build_quit_category(),
    ]
}

fn build_build_indices_category(config: &Config) -> Category {
    let mut commands = vec![
        Command {
            label: "Scan Corpus".to_string(),
            action: CommandAction::Background(BackgroundTask::ScanCorpus { re_fingerprint: false }),
            description: "Incrementally scan corpus, skipping unchanged files".to_string(),
        },
        Command {
            label: "Scan Corpus (re-fingerprint)".to_string(),
            action: CommandAction::Background(BackgroundTask::ScanCorpus { re_fingerprint: true }),
            description: "Full rescan of corpus, recomputing all fingerprints".to_string(),
        },
    ];

    // Add legacy corpus scan if configured
    if config.legacy_library.is_some() {
        commands.push(Command {
            label: "Scan Legacy Corpus".to_string(),
            action: CommandAction::Background(BackgroundTask::ScanLegacy),
            description: "Scan the legacy library for matching/migration".to_string(),
        });
    }

    Category {
        name: "Build Indices".to_string(),
        commands,
    }
}

fn build_insight_category() -> Category {
    Category {
        name: "Insight & Health".to_string(),
        commands: vec![
            Command {
                label: "Generate All Reports".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::GenerateAll,
                }),
                description: "TODO: Generate all configured reports".to_string(),
            },
            Command {
                label: "Legacy Library Report".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Legacy,
                }),
                description: "TODO: Report on legacy library coverage".to_string(),
            },
            Command {
                label: "Corpus Deployment Report".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Deployment,
                }),
                description: "TODO: Report on deployment status".to_string(),
            },
            Command {
                label: "Quality Report (Canonicalization)".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Quality,
                }),
                description: "TODO: Report on metadata quality".to_string(),
            },
            Command {
                label: "Duplicate Detection Report".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Duplicates,
                }),
                description: "TODO: Report on detected duplicates".to_string(),
            },
        ],
    }
}

fn build_corpus_ops_category() -> Category {
    Category {
        name: "Corpus-mutating Operations".to_string(),
        commands: vec![
            Command {
                label: "Metadata Deduplication (Tag Editor)".to_string(),
                action: CommandAction::Transition(TransitionTarget::TagEditor),
                description: "TODO: Resolve metadata conflicts across duplicate tracks".to_string(),
            },
            Command {
                label: "Fingerprint Deduplication (Decision Flow)".to_string(),
                action: CommandAction::Transition(TransitionTarget::DecisionFlow),
                description: "TODO: Resolve audio fingerprint duplicates".to_string(),
            },
        ],
    }
}

fn build_deployment_category() -> Category {
    Category {
        name: "Deployment".to_string(),
        commands: vec![
            Command {
                label: "Preview Changes (Dry Run)".to_string(),
                action: CommandAction::Background(BackgroundTask::Deploy { dry_run: true }),
                description: "TODO: Preview deployment without making changes".to_string(),
            },
            Command {
                label: "Deploy to Libraries".to_string(),
                action: CommandAction::Background(BackgroundTask::Deploy { dry_run: false }),
                description: "TODO: Create hard links in library directories".to_string(),
            },
            Command {
                label: "View Pending Changes".to_string(),
                action: CommandAction::Transition(TransitionTarget::PendingChangesView),
                description: "TODO: Review queued changes before commit".to_string(),
            },
        ],
    }
}

fn build_intake_category() -> Category {
    Category {
        name: "Intake".to_string(),
        commands: vec![Command {
            label: "(Coming Soon)".to_string(),
            action: CommandAction::Message("Intake workflow not yet implemented".to_string()),
            description: "TODO: Import external material into corpus".to_string(),
        }],
    }
}

fn build_quit_category() -> Category {
    Category {
        name: "Quit".to_string(),
        commands: vec![Command {
            label: "Exit MLA".to_string(),
            action: CommandAction::Quit,
            description: "Exit the application".to_string(),
        }],
    }
}
