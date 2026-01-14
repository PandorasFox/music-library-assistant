//! Multi-pane Main Menu Navigation
//!
//! **TODO: DEPRECATED MODULE**
//!
//! This module provided the main menu UI, which has been replaced by the Insights
//! view as the primary interface. The following types may still be in use:
//! - `CommandAction` - action dispatch enum for menu commands
//! - `TransitionTarget` - target modes for state transitions
//! - `BackgroundTask` / `ReportType` - background operation identifiers
//! - `DirBrowserContext` - context for directory browser launches
//! - `MainMenuState` - may be removable (state updates now go unused)
//!
//! Review call sites in mod.rs for actual usage. Most menu-based flows are now
//! inaccessible until wired through the Insights view's flow launcher.
//!
//! Original description:
//! Provides a three-column navigation system:
//! - Left pane: Categories (Build Indices, Insight, Corpus Ops, etc.)
//! - Middle pane: Commands within selected category
//! - Right pane: Info/description for highlighted command

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::ui::widgets::{PaneConfig, ThreePaneLayout, TwoPaneLayout};

use crate::config::Config;
use crate::corpus::HeartbeatResult;
use crate::corpus::db::CorpusSummary;

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
    /// Cached corpus summary for info panel display
    pub corpus_summary: Option<CorpusSummary>,
    /// Startup heartbeat result
    pub heartbeat_result: Option<HeartbeatResult>,
}

/// Category definition with commands
#[derive(Debug, Clone)]
pub struct Category {
    pub name: String,
    /// Commands within this category (for normal categories)
    pub commands: Vec<Command>,
    /// Direct action for "action categories" - when set, selecting this category
    /// executes the action immediately without showing a submenu.
    /// The right pane shows as a single empty area.
    /// Example: "Quit" is an action category.
    pub action: Option<CommandAction>,
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
    ScanCorpus,
    /// Scan the legacy library (if configured)
    ScanLegacy,
    GenerateReport { report_type: ReportType },
    /// Deploy to libraries (opens preview, then executes)
    Deploy,
    /// Stub for unimplemented features (shows "not yet implemented" message)
    Stub,
}

/// Context for what operation uses the directory browser.
#[derive(Debug, Clone, Copy)]
pub enum DirBrowserContext {
    /// Fingerprint-based deduplication
    Sleuthing,
}

/// Target UI modes for transitions
#[derive(Debug, Clone, Copy)]
pub enum TransitionTarget {
    TagEditor,
    DecisionFlow,
    DirBrowser { context: DirBrowserContext },
    PendingChangesView,
    DropMissingConfirm,
    /// Artist name canonicalization flow
    CanonFlow,
    /// Genre tag canonicalization flow
    GenreCanonFlow,
    /// Corpus browser with metadata preview
    CorpusBrowser,
    /// Album artist resolution flow (phase selector)
    AlbumArtistFlow,
    /// Album tag resolution flow
    AlbumFlow,
    /// Corpus insights view (lateral view ring)
    Insights,
}

/// Report types
///
/// Note: Quality and Duplicates reports were removed as redundant.
/// The Health report now covers all duplicate and canonicalization issues.
#[derive(Debug, Clone, Copy)]
pub enum ReportType {
    GenerateAll,
    Legacy,
    Deployment,
    Health,
    KnownVariants,
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
            corpus_summary: None,
            heartbeat_result: None,
        }
    }

    /// Update the cached corpus summary
    pub fn set_corpus_summary(&mut self, summary: CorpusSummary) {
        self.corpus_summary = Some(summary);
    }

    /// Update the heartbeat result
    pub fn set_heartbeat_result(&mut self, result: HeartbeatResult) {
        self.heartbeat_result = Some(result);
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
            // Don't move to command pane for action categories (they have no commands)
            let is_action_category = self
                .current_category()
                .map(|c| c.action.is_some())
                .unwrap_or(false);

            if !is_action_category {
                self.focus = PaneFocus::Command;
                // Ensure command is selected
                self.command_list_state.select(Some(self.command_index));
            }
        }
    }

    fn execute_selection(&mut self) -> MenuAction {
        match self.focus {
            PaneFocus::Category => {
                // Check if this is an action category (has direct action, no submenu)
                if let Some(category) = self.current_category() {
                    if let Some(ref action) = category.action {
                        // Action category: execute immediately
                        return MenuAction::Execute(action.clone());
                    }
                }
                // Normal category: move to commands pane
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
        // Check if current category is an action category (no submenu)
        let is_action_category = self
            .current_category()
            .map(|c| c.action.is_some())
            .unwrap_or(false);

        if is_action_category {
            // Two-pane layout: categories on left, single empty pane on right
            let layout = TwoPaneLayout::horizontal()
                .left(PaneConfig::new("", 25))
                .right(PaneConfig::new("", 75))
                .build(area);

            self.render_categories(f, layout.left.area);
            self.render_action_category_pane(f, layout.right.area);
        } else {
            // Normal three-pane layout
            let layout = ThreePaneLayout::horizontal()
                .left(PaneConfig::new("", 25))
                .middle(PaneConfig::new("", 35))
                .right(PaneConfig::new("", 40))
                .build(area);

            self.render_categories(f, layout.left.area);
            self.render_commands(f, layout.middle.area);
            self.render_info(f, layout.right.area);
        }
    }

    /// Render an empty pane for action categories (like Quit)
    fn render_action_category_pane(&self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL);
        f.render_widget(block, area);
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
            // Exit gets an empty info panel
            if matches!(command.action, CommandAction::Quit) {
                return lines;
            }

            lines.push(command.description.clone());
            lines.push(String::new());

            // Add context-specific info based on command action
            match &command.action {
                CommandAction::Background(BackgroundTask::ScanCorpus) => {
                    lines.push("Uses cached timestamps for incremental scan.".to_string());
                    lines.push("Only processes new/modified files.".to_string());
                    // Show last scan time if available
                    if let Some(ref summary) = self.corpus_summary {
                        if let Some(ref last_scan) = summary.last_scan {
                            lines.push(String::new());
                            lines.push(format!("Last scan: {}", last_scan));
                        }
                    }
                }
                CommandAction::Background(BackgroundTask::ScanLegacy) => {
                    lines.push("Scans legacy library for migration analysis.".to_string());
                }
                CommandAction::Background(BackgroundTask::GenerateReport { report_type }) => {
                    // Show report-specific summaries
                    self.add_report_summary(&mut lines, report_type);
                }
                CommandAction::Transition(target) => {
                    let desc = match target {
                        TransitionTarget::TagEditor => "Opens interactive tag editor",
                        TransitionTarget::DecisionFlow => "Opens decision-making workflow",
                        TransitionTarget::DirBrowser { .. } => "Opens directory browser for path selection",
                        TransitionTarget::DropMissingConfirm => "Shows index entries without backing files for removal",
                        TransitionTarget::PendingChangesView => "Shows queued changes",
                        TransitionTarget::CanonFlow => "Opens artist canonicalization workflow",
                        TransitionTarget::GenreCanonFlow => "Opens genre canonicalization workflow",
                        TransitionTarget::CorpusBrowser => "Browse corpus files with metadata preview",
                        TransitionTarget::AlbumArtistFlow => "Unified album artist resolution workflow",
                        TransitionTarget::AlbumFlow => "Album tag canonicalization with EP detection",
                        TransitionTarget::Insights => "Real-time computed insights over health signals",
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

    /// Add report-specific summary based on report type
    fn add_report_summary(&self, lines: &mut Vec<String>, report_type: &ReportType) {
        let Some(ref summary) = self.corpus_summary else {
            lines.push("Run a scan to populate statistics.".to_string());
            return;
        };

        let hs = &summary.health_summary;

        match report_type {
            ReportType::Health => {
                lines.push("─── Current Health ───".to_string());
                lines.push(format!("Deploy conflicts: {}", summary.deploy_conflicts));
                lines.push(format!("  Fingerprint duplicates: {}", hs.fingerprint_duplicates));
                lines.push(format!("Canonicalization issues: {}", hs.canonicalization_issues));
                lines.push(format!("Auto-resolvable: {}", hs.auto_resolvable));
                lines.push(format!("Manual review: {}", hs.manual_review));
                lines.push(format!("Known variants: {}", hs.known_variants));
            }
            ReportType::Deployment => {
                lines.push("─── Deployment Status ───".to_string());
                if let Some(ref ds) = summary.deployment_stats {
                    lines.push(format!("Corpus files: {}", ds.total_corpus_files));
                    lines.push(format!("Deployed: {}", ds.deployed_files));
                    lines.push(format!("Coverage: {:.1}%", ds.deployment_percentage));
                } else {
                    lines.push("No deployment stats available.".to_string());
                }
            }
            ReportType::Legacy => {
                lines.push("─── Legacy Analysis ───".to_string());
                lines.push("Compares legacy library against corpus.".to_string());
                lines.push("Identifies migration candidates.".to_string());
            }
            ReportType::GenerateAll => {
                lines.push("Generates all configured reports:".to_string());
                lines.push("  - Health status (duplicates + canonicalization)".to_string());
                lines.push("  - Deployment coverage".to_string());
                lines.push("  - Known variants".to_string());
            }
            ReportType::KnownVariants => {
                lines.push("─── Known Variants ───".to_string());
                lines.push(format!("Stored variants: {}", hs.known_variants));
                lines.push("Lists all explicitly confirmed artist/album variations.".to_string());
            }
        }
    }
}

// ============================================================================
// Menu Building
// ============================================================================

fn build_menu_categories(_config: &Config) -> Vec<Category> {
    // Note: Build Indices category removed - scanning is now automatic:
    // - Corpus: First-time startup auto-scans when DB is empty; heartbeat scans new files
    // - Legacy: Heartbeat scans legacy library if configured
    vec![
        build_insight_category(),
        build_corpus_ops_category(),
        build_deployment_category(),
        build_intake_category(),
        build_quit_category(),
    ]
}

fn build_insight_category() -> Category {
    // Get health summary for description
    let health_description = match crate::ops::reports::get_health_summary() {
        Ok(summary) => crate::ops::reports::format_health_summary_brief(&summary),
        Err(_) => "Health status unavailable - scan corpus first".to_string(),
    };

    Category {
        name: "Insight & Health".to_string(),
        commands: vec![
            Command {
                label: "Corpus Insights".to_string(),
                action: CommandAction::Transition(TransitionTarget::Insights),
                description: "View real-time computed insights over health signals".to_string(),
            },
            Command {
                label: "Health Status".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Health,
                }),
                description: health_description,
            },
            Command {
                label: "Generate All Reports".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::GenerateAll,
                }),
                description: "Run all configured health and status reports".to_string(),
            },
            Command {
                label: "Legacy Library Report".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Legacy,
                }),
                description: "Compare legacy library coverage against corpus".to_string(),
            },
            Command {
                label: "Corpus Deployment Report".to_string(),
                action: CommandAction::Background(BackgroundTask::GenerateReport {
                    report_type: ReportType::Deployment,
                }),
                description: "Show deployment status across all libraries".to_string(),
            },
        ],
        action: None,
    }
}

// =============================================================================
// TODO: Album Artist Health Restoration - Design Intentions
// =============================================================================
//
// The three album artist flows below should eventually be unified into a single
// "Album Artist Health Restoration" meta-flow that guides the operator through
// all three sub-flows in sequence.
//
// ## Design Goals
//
// 1. **Transformed State Reasoning**: As the operator proceeds through each flow
//    and accumulates mutations, we should reason about the "transformed" corpus
//    state in-memory. This means applying pending mutations virtually when
//    computing health metrics for subsequent flows.
//
// 2. **Progressive Resolution**: Each flow should see the corpus as it WILL BE
//    after earlier flows' mutations are applied, not as it currently exists.
//    This prevents redundant work and shows accurate resolution state.
//
// 3. **3-Pane/3-Stage Review**: Consider a review process that shows:
//    - Pane 1: Capitalization canonicalizations (album_artist case variants)
//    - Pane 2: Album artist inference (from album+track# patterns)
//    - Pane 3: Album artist population (from artist field where missing)
//    Each pane could be independently reviewed before final commit.
//
// 4. **Final Resolution Display**: After all three flows, display a summary
//    showing the overall transformed state - how many tracks were affected,
//    what the album_artist distribution looks like post-mutation, etc.
//
// ## Health Metrics Needed
//
// - Album artist capitalization variants (similar to artist canon buckets)
// - Tracks with album + track_number but no album_artist (grouped by album similarity)
// - Tracks with album + artist but no album_artist (candidates for population)
// - Album coherence score (do all tracks in an "album" agree on album_artist?)
//
// ## Implementation Notes
//
// - May want to share mutation accumulation infrastructure with canon_flow
// - Virtual corpus state could use a HashMap<track_id, PendingChange> overlay
// - Review panes could use similar patterns to CanonSessionReview
// - Consider "back" navigation between flows (not just within)
//
// =============================================================================

fn build_corpus_ops_category() -> Category {
    Category {
        name: "Corpus-mutating Operations".to_string(),
        commands: vec![
            // -----------------------------------------------------------------
            // Corpus Browser - browse and edit tags in-place
            // -----------------------------------------------------------------
            Command {
                label: "Corpus Browser".to_string(),
                action: CommandAction::Transition(TransitionTarget::CorpusBrowser),
                description: "Browse corpus files and edit tags with metadata preview".to_string(),
            },
            // -----------------------------------------------------------------
            // Index Maintenance - drop entries for files no longer on disk
            // -----------------------------------------------------------------
            Command {
                label: "Drop Unbacked Files From Corpus Indices".to_string(),
                action: CommandAction::Transition(TransitionTarget::DropMissingConfirm),
                description: "Remove index entries for files that no longer exist on disk".to_string(),
            },
            Command {
                label: "Deploy Conflict Resolution".to_string(),
                action: CommandAction::Transition(TransitionTarget::TagEditor),
                description: "Resolve tracks that would deploy to the same library path".to_string(),
            },
            Command {
                label: "Scoped Fingerprint Deduplication".to_string(),
                action: CommandAction::Transition(TransitionTarget::DirBrowser {
                    context: DirBrowserContext::Sleuthing,
                }),
                description: "Select directories and resolve exact audio duplicates".to_string(),
            },
            Command {
                label: "Artist Name Canonicalization".to_string(),
                action: CommandAction::Transition(TransitionTarget::CanonFlow),
                description: "Resolve artist name spelling variants".to_string(),
            },
            // -----------------------------------------------------------------
            // Album Artist Resolution - STUBBED OUT FOR REDESIGN
            // TODO: Needs complete redesign of gathering logic and UI presentation
            // -----------------------------------------------------------------
            Command {
                label: "Album Artist Resolution".to_string(),
                action: CommandAction::Message("Stubbed - needs redesign".to_string()),
                description: "STUB: canonicalization, collation, population (disabled)".to_string(),
            },
            // -----------------------------------------------------------------
            // Album Tag Resolution Flow
            // Handles album tag variants (EP/Deluxe/Remaster suffixes) with
            // fingerprint overlap detection for near-match grouping.
            // -----------------------------------------------------------------
            Command {
                label: "Album Tag Resolution".to_string(),
                action: CommandAction::Transition(TransitionTarget::AlbumFlow),
                description: "Resolve album tag variants (EP/Deluxe/Remaster suffixes)".to_string(),
            },
            Command {
                label: "Genre Tag Canonicalization".to_string(),
                action: CommandAction::Transition(TransitionTarget::GenreCanonFlow),
                description: "Resolve genre spelling variants (Hip Hop vs Hip-Hop)".to_string(),
            },
        ],
        action: None,
    }
}

fn build_deployment_category() -> Category {
    Category {
        name: "Deployment".to_string(),
        commands: vec![
            Command {
                label: "Deploy to Libraries".to_string(),
                action: CommandAction::Background(BackgroundTask::Deploy),
                description: "Preview deployment status and create hard links".to_string(),
            },
            Command {
                label: "View Pending Changes".to_string(),
                action: CommandAction::Transition(TransitionTarget::PendingChangesView),
                description: "Preview and commit pending corpus changes".to_string(),
            },
        ],
        action: None,
    }
}

fn build_intake_category() -> Category {
    Category {
        name: "Intake".to_string(),
        commands: vec![Command {
            label: "(Coming Soon)".to_string(),
            action: CommandAction::Message("Intake workflow not yet implemented".to_string()),
            description: "Scan and import new material into corpus".to_string(),
        }],
        action: None,
    }
}

/// Build an "action category" - a top-level menu entry that executes immediately
/// when selected (no submenu). Shows a single empty pane to the right.
fn build_quit_category() -> Category {
    Category {
        name: "Quit".to_string(),
        commands: vec![], // Action categories have no submenu
        action: Some(CommandAction::Quit),
    }
}
