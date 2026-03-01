//! Embed Album Art Resolution Modal
//!
//! Shows directories with sidecar images alongside artless audio files.
//! Offers a one-click embed to insert the sidecar image into all artless files.
//!
//! - Up/Down: Navigate directory list
//! - Enter: Confirm embed all
//! - Escape: Cancel

use crate::ui::input::InputAction;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::album_art::EmbedAlbumArtMutation;
use crate::meta::signals::data::EmbeddableAlbumArtSignal;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::control_colors as cc;
use crate::ui::widgets::{
    AlbumArtCache, AlbumArtPicker, ConfirmationButton,
    render_album_art_preview, render_button_row, render_no_art_placeholder,
};

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

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> EmbedAlbumArtPreviewAction {
        match action {
            InputAction::Cancel => EmbedAlbumArtPreviewAction::Cancel,

            InputAction::Confirm => EmbedAlbumArtPreviewAction::ConfirmEmbedAll,

            InputAction::NavUp => {
                if self.scroll > 0 {
                    self.scroll -= 1;
                }
                EmbedAlbumArtPreviewAction::None
            }

            InputAction::NavDown => {
                if self.scroll + 1 < self.cached_data.directory_count() {
                    self.scroll += 1;
                }
                EmbedAlbumArtPreviewAction::None
            }

            InputAction::Home => {
                self.scroll = 0;
                EmbedAlbumArtPreviewAction::None
            }

            InputAction::End => {
                self.scroll = self.cached_data.directory_count().saturating_sub(1);
                EmbedAlbumArtPreviewAction::None
            }

            _ => EmbedAlbumArtPreviewAction::None,
        }
    }

    /// Render the modal with album art preview.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
    ) {
        // Clear background
        f.render_widget(Clear, area);

        // Split into title + content + buttons + hints
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // title
                Constraint::Min(5),    // content
                Constraint::Length(1), // buttons
                Constraint::Length(1), // hints
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

        // Content: split into directory list (left) + art preview (right)
        let content_area = chunks[1];
        let has_preview = art_picker.is_available() && content_area.width > 40;

        if has_preview {
            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(30),          // directory list
                    Constraint::Length(content_area.height.saturating_mul(2).max(20)), // art preview (roughly square)
                ])
                .split(content_area);

            self.render_directory_list(f, content_chunks[0]);
            self.render_art_preview(f, content_chunks[1], art_picker, art_cache);
        } else {
            self.render_directory_list(f, content_area);
        }

        // Buttons
        let buttons = [
            ConfirmationButton::new("Confirm", Color::Green).selected(true),
        ];
        render_button_row(f, chunks[2], &buttons);

        // Hints
        let hints = Paragraph::new(Line::from(vec![
            cc::nav("[↑↓]"),
            cc::text(" navigate  "),
            cc::confirm("[Enter]"),
            cc::text(" confirm  "),
            cc::cancel("[Esc]"),
            cc::text(" cancel"),
        ]))
        .alignment(Alignment::Center);
        f.render_widget(hints, chunks[3]);
    }

    /// Render the directory list (left pane).
    fn render_directory_list(&self, f: &mut Frame, area: Rect) {
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

        f.render_widget(list, area);
    }

    /// Render the album art preview (right pane).
    fn render_art_preview(
        &self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Preview ")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.width < 2 || inner.height < 2 {
            return;
        }

        // Get the image path of the currently selected signal
        if let Some(signal) = self.cached_data.signals.get(self.scroll) {
            let image_path = Path::new(&signal.data.image_path);

            // Evict stale cache entries (keep only the current image)
            art_cache.retain_only(&[image_path]);

            let cached = art_cache.get_or_load(image_path, art_picker);

            // Show image metadata below the preview
            let (preview_area, info_area) = if inner.height > 4 {
                let split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),     // image
                        Constraint::Length(1),   // metadata line
                    ])
                    .split(inner);
                (split[0], Some(split[1]))
            } else {
                (inner, None)
            };

            render_album_art_preview(f, preview_area, cached);

            // Show metadata line: "cover.png 1200x1200 PNG"
            if let Some(info_area) = info_area {
                let info_text = if cached.width > 0 {
                    format!(
                        "{} {}x{} {}",
                        cached.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                        cached.width,
                        cached.height,
                        cached.format.to_uppercase(),
                    )
                } else {
                    cached.path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                };
                let info_line = Line::from(Span::styled(
                    info_text,
                    Style::default().fg(Color::DarkGray),
                ));
                f.render_widget(
                    Paragraph::new(info_line).alignment(Alignment::Center),
                    info_area,
                );
            }
        } else {
            render_no_art_placeholder(f, inner);
        }
    }
}
