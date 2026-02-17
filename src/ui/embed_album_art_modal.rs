//! Embed Album Art Resolution Modal
//!
//! Shows directories with sidecar images alongside artless audio files.
//! Offers a one-click embed to insert the sidecar image into all artless files.
//!
//! - Up/Down: Navigate directory list
//! - Enter: Confirm embed all
//! - Escape: Cancel

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use anyhow::Result;
use std::path::PathBuf;

use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::album_art::EmbedAlbumArtMutation;
use crate::meta::signals::data::EmbeddableAlbumArtSignal;
use crate::ui::helpers::render_pane;

/// Actions returned from the embed album art preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedAlbumArtPreviewAction {
    /// No action needed.
    None,
    /// User confirmed embed all.
    ConfirmEmbedAll,
    /// Cancel and return to Insights view.
    Cancel,
}

/// Cached data for the embed album art resolution modal.
#[derive(Debug, Clone)]
pub struct EmbedAlbumArtModalData {
    /// Loaded signals with deserialized data.
    pub signals: Vec<EmbeddableAlbumArtSignal>,
}

impl Default for EmbedAlbumArtModalData {
    fn default() -> Self {
        Self {
            signals: Vec::new(),
        }
    }
}

impl EmbedAlbumArtModalData {
    /// Load embeddable album art signals from the database.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        let signals = read_db.get_embeddable_album_art_signals()?;
        Ok(Self { signals })
    }

    /// Total number of directories with embeddable art.
    pub fn directory_count(&self) -> usize {
        self.signals.len()
    }

    /// Total number of artless files across all directories.
    pub fn total_artless_files(&self) -> usize {
        self.signals.iter().map(|s| s.data.artless_inodes.len()).sum()
    }

    /// Generate embed mutations for all directories.
    pub fn embed_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        for signal in &self.signals {
            let image_path = PathBuf::from(&signal.data.image_path);
            for (i, inode) in signal.data.artless_inodes.iter().enumerate() {
                let rel_path = &signal.data.artless_paths[i];
                let audio_path = resolver.resolve(std::path::Path::new(rel_path));

                mutations.push(Mutation::EmbedAlbumArt(EmbedAlbumArtMutation {
                    inode: *inode,
                    audio_path,
                    image_path: image_path.clone(),
                    signal_key: signal.key.clone(),
                }));
            }
        }

        mutations
    }
}

/// State for the embed album art resolution modal.
#[derive(Debug)]
pub struct EmbedAlbumArtPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: EmbedAlbumArtModalData,
    /// Scroll position for the directory list.
    pub scroll: usize,
}

impl EmbedAlbumArtPreviewState {
    /// Create a new preview state.
    pub fn new(cached_data: EmbedAlbumArtModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
        }
    }

    /// Path of the currently selected directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data.signals.get(self.scroll).map(|s| s.key.as_str())
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> EmbedAlbumArtPreviewAction {
        match key.code {
            KeyCode::Esc => EmbedAlbumArtPreviewAction::Cancel,

            KeyCode::Enter => EmbedAlbumArtPreviewAction::ConfirmEmbedAll,

            KeyCode::Up => {
                if self.scroll > 0 {
                    self.scroll -= 1;
                }
                EmbedAlbumArtPreviewAction::None
            }

            KeyCode::Down => {
                if self.scroll + 1 < self.cached_data.directory_count() {
                    self.scroll += 1;
                }
                EmbedAlbumArtPreviewAction::None
            }

            KeyCode::Home => {
                self.scroll = 0;
                EmbedAlbumArtPreviewAction::None
            }

            KeyCode::End => {
                self.scroll = self.cached_data.directory_count().saturating_sub(1);
                EmbedAlbumArtPreviewAction::None
            }

            _ => EmbedAlbumArtPreviewAction::None,
        }
    }

    /// Render the modal.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Split into title + content + controls
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // title
                Constraint::Min(5),    // content
                Constraint::Length(3), // controls
            ])
            .split(area);

        // Title
        let title_text = format!(
            " Embed album art: {} directories, {} artless files ",
            self.cached_data.directory_count(),
            self.cached_data.total_artless_files(),
        );
        let title_block = Block::default()
            .borders(Borders::ALL)
            .title(title_text)
            .border_style(Style::default().fg(Color::Cyan));
        render_pane(f, chunks[0], title_block);

        // Content: list of directories
        let list_items: Vec<ListItem> = self
            .cached_data
            .signals
            .iter()
            .enumerate()
            .map(|(i, signal)| {
                let is_selected = i == self.scroll;
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let text = format!(
                    "  {} — {} ({} files)",
                    signal.key,
                    signal.data.image_filename,
                    signal.data.artless_inodes.len(),
                );
                ListItem::new(Line::from(Span::styled(text, style)))
            })
            .collect();

        let list = List::new(list_items)
            .block(Block::default().borders(Borders::ALL).title(" Directories "));

        f.render_widget(list, chunks[1]);

        // Controls
        let controls = Paragraph::new(Line::from(vec![
            Span::styled(
                " [Enter] ",
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Embed all  ", Style::default().fg(Color::White)),
            Span::styled(
                " [Esc] ",
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Cancel ", Style::default().fg(Color::White)),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(controls, chunks[2]);
    }
}
