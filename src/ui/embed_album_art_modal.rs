//! Album Art Review Modal
//!
//! Per-directory review of album art work: embedding sidecar images into artless
//! audio files and upgrading lower-quality embedded art with better sidecars.
//!
//! - Up/Down: Scroll file list within current directory
//! - Left/Right: Switch between Confirm/Skip buttons
//! - Enter: Confirm or skip current directory, advance to next
//! - Escape: Cancel entire review

use crate::ui::input::InputAction;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::corpus::paths;
use crate::db::ReadOnlyDb;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::album_art::{EmbedAlbumArtMutation, UpgradeAlbumArtMutation};
use crate::meta::signals::data::SidecarImage;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::control_colors as cc;
use crate::ui::widgets::{
    AlbumArtCache, AlbumArtPicker, ConfirmationButton,
    render_album_art_preview, render_button_row, render_no_art_placeholder,
};

// ============================================================================
// Actions
// ============================================================================

/// Actions returned from the album art review modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumArtReviewAction {
    /// No action needed.
    None,
    /// Confirm current directory (embed + upgrade its files).
    ConfirmDirectory,
    /// Skip current directory without staging.
    SkipDirectory,
    /// Cancel entire review.
    Cancel,
}

// ============================================================================
// Data Types
// ============================================================================

/// A file that needs album art work within a directory.
#[derive(Debug, Clone)]
pub struct ArtReviewFile {
    pub inode: i64,
    /// Corpus-relative path.
    pub path: String,
    /// For upgrade entries: description of current embedded art.
    pub current_art_desc: Option<String>,
}

/// A directory that has album art work to do.
#[derive(Debug, Clone)]
pub struct ArtReviewDirectory {
    /// Directory relative path (signal key).
    pub directory: String,
    /// Primary sidecar image (best quality, used for embed/upgrade).
    pub sidecar: SidecarImage,
    /// All sidecar images found (for preview/selection).
    pub all_sidecars: Vec<SidecarImage>,
    /// Signal key for embeddable signal (if any).
    pub embed_signal_key: Option<String>,
    /// Signal key for upgradeable signal (if any).
    pub upgrade_signal_key: Option<String>,
    /// Files that need art embedded (currently artless).
    pub embed_entries: Vec<ArtReviewFile>,
    /// Files that could be upgraded (have lower-quality art).
    pub upgrade_entries: Vec<ArtReviewFile>,
}

impl ArtReviewDirectory {
    /// Generate mutations for this directory.
    pub fn mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let image_path = PathBuf::from(&self.sidecar.path);
        let mut mutations = Vec::new();

        // Embed mutations
        if let Some(ref signal_key) = self.embed_signal_key {
            for entry in &self.embed_entries {
                let audio_path = resolver.resolve(Path::new(&entry.path));
                mutations.push(Mutation::EmbedAlbumArt(EmbedAlbumArtMutation {
                    inode: entry.inode,
                    audio_path,
                    image_path: image_path.clone(),
                    signal_key: signal_key.clone(),
                }));
            }
        }

        // Upgrade mutations
        if let Some(ref signal_key) = self.upgrade_signal_key {
            for entry in &self.upgrade_entries {
                let audio_path = resolver.resolve(Path::new(&entry.path));
                mutations.push(Mutation::UpgradeAlbumArt(UpgradeAlbumArtMutation {
                    inode: entry.inode,
                    audio_path,
                    image_path: image_path.clone(),
                    signal_key: signal_key.clone(),
                    current_art_desc: entry.current_art_desc.clone().unwrap_or_default(),
                }));
            }
        }

        mutations
    }
}

// ============================================================================
// Data Loading
// ============================================================================

/// Load and merge embeddable + upgradeable signals into per-directory review data.
pub fn load_review_directories(read_db: &ReadOnlyDb<'_>) -> Result<Vec<ArtReviewDirectory>> {
    let embed_signals = read_db.get_embeddable_album_art_signals().unwrap_or_default();
    let upgrade_signals = read_db.get_upgradeable_album_art_signals().unwrap_or_default();

    // Merge by directory key. Use BTreeMap for stable ordering.
    let mut dirs: BTreeMap<String, ArtReviewDirectory> = BTreeMap::new();

    for signal in embed_signals {
        let dir_key = signal.key.clone();
        let entry = dirs.entry(dir_key.clone()).or_insert_with(|| ArtReviewDirectory {
            directory: dir_key,
            sidecar: signal.data.sidecar_images.first().cloned().unwrap_or(SidecarImage {
                path: signal.data.image_path.clone(),
                filename: signal.data.image_filename.clone(),
                role: crate::meta::signals::data::PictureRole::CoverFront,
                format: String::new(),
                width: 0,
                height: 0,
            }),
            all_sidecars: signal.data.sidecar_images.clone(),
            embed_signal_key: None,
            upgrade_signal_key: None,
            embed_entries: Vec::new(),
            upgrade_entries: Vec::new(),
        });

        entry.embed_signal_key = Some(signal.key.clone());
        for (i, inode) in signal.data.artless_inodes.iter().enumerate() {
            entry.embed_entries.push(ArtReviewFile {
                inode: *inode,
                path: signal.data.artless_paths[i].clone(),
                current_art_desc: None,
            });
        }
    }

    for signal in upgrade_signals {
        let dir_key = signal.key.strip_prefix("upgrade:").unwrap_or(&signal.key).to_string();
        let art_desc = format!(
            "{}x{} {}",
            signal.data.embedded_width,
            signal.data.embedded_height,
            signal.data.embedded_format,
        );

        let entry = dirs.entry(dir_key.clone()).or_insert_with(|| ArtReviewDirectory {
            directory: dir_key,
            sidecar: signal.data.sidecar.clone(),
            all_sidecars: vec![signal.data.sidecar.clone()],
            embed_signal_key: None,
            upgrade_signal_key: None,
            embed_entries: Vec::new(),
            upgrade_entries: Vec::new(),
        });

        entry.upgrade_signal_key = Some(signal.key.clone());
        // If this directory didn't have embed signals, use upgrade's sidecar
        if entry.all_sidecars.is_empty() {
            entry.sidecar = signal.data.sidecar.clone();
            entry.all_sidecars = vec![signal.data.sidecar.clone()];
        }

        for (i, inode) in signal.data.upgradeable_inodes.iter().enumerate() {
            entry.upgrade_entries.push(ArtReviewFile {
                inode: *inode,
                path: signal.data.upgradeable_paths[i].clone(),
                current_art_desc: Some(art_desc.clone()),
            });
        }
    }

    Ok(dirs.into_values().collect())
}

// ============================================================================
// Review State
// ============================================================================

/// Which button is focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumArtReviewButton {
    Confirm,
    Skip,
}

/// State for the per-directory album art review modal.
#[derive(Debug)]
pub struct AlbumArtReviewState {
    /// All directories to review.
    pub directories: Vec<ArtReviewDirectory>,
    /// Index of current directory being reviewed.
    pub current_dir: usize,
    /// File list scroll offset within current directory.
    pub file_scroll: usize,
    /// Which button is selected.
    pub selected_button: AlbumArtReviewButton,
    /// Accumulated mutations from confirmed directories.
    pub staged_mutations: Vec<Mutation>,
    /// Number of directories confirmed so far.
    pub confirmed_count: usize,
}

impl AlbumArtReviewState {
    /// Create a new review state.
    pub fn new(directories: Vec<ArtReviewDirectory>) -> Self {
        Self {
            directories,
            current_dir: 0,
            file_scroll: 0,
            selected_button: AlbumArtReviewButton::Confirm,
            staged_mutations: Vec::new(),
            confirmed_count: 0,
        }
    }

    /// Whether all directories have been reviewed.
    pub fn is_complete(&self) -> bool {
        self.current_dir >= self.directories.len()
    }

    /// Get the current directory, if any.
    fn current(&self) -> Option<&ArtReviewDirectory> {
        self.directories.get(self.current_dir)
    }

    /// Total line count for file list in current directory.
    fn file_list_len(&self) -> usize {
        self.current().map(|d| {
            let mut count = 0;
            if !d.embed_entries.is_empty() {
                count += 1 + d.embed_entries.len(); // header + files
            }
            if !d.upgrade_entries.is_empty() {
                count += 1 + d.upgrade_entries.len() * 2; // header + (file + desc) pairs
            }
            count
        }).unwrap_or(0)
    }

    /// Advance to next directory, resetting scroll.
    pub fn advance(&mut self) {
        self.current_dir += 1;
        self.file_scroll = 0;
        self.selected_button = AlbumArtReviewButton::Confirm;
    }

    /// Path of the currently selected directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.current().map(|d| d.directory.as_str())
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> AlbumArtReviewAction {
        match action {
            InputAction::Cancel => AlbumArtReviewAction::Cancel,

            InputAction::Confirm => {
                match self.selected_button {
                    AlbumArtReviewButton::Confirm => AlbumArtReviewAction::ConfirmDirectory,
                    AlbumArtReviewButton::Skip => AlbumArtReviewAction::SkipDirectory,
                }
            }

            InputAction::NavLeft => {
                self.selected_button = AlbumArtReviewButton::Confirm;
                AlbumArtReviewAction::None
            }

            InputAction::NavRight => {
                self.selected_button = AlbumArtReviewButton::Skip;
                AlbumArtReviewAction::None
            }

            InputAction::NavUp => {
                if self.file_scroll > 0 {
                    self.file_scroll -= 1;
                }
                AlbumArtReviewAction::None
            }

            InputAction::NavDown => {
                let max = self.file_list_len().saturating_sub(1);
                if self.file_scroll < max {
                    self.file_scroll += 1;
                }
                AlbumArtReviewAction::None
            }

            InputAction::Home => {
                self.file_scroll = 0;
                AlbumArtReviewAction::None
            }

            InputAction::End => {
                self.file_scroll = self.file_list_len().saturating_sub(1);
                AlbumArtReviewAction::None
            }

            _ => AlbumArtReviewAction::None,
        }
    }

    /// Render the per-directory review modal.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
    ) {
        f.render_widget(Clear, area);

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
            " Album Art Review [{}/{}] ",
            self.current_dir + 1,
            self.directories.len(),
        );
        let title_block = Block::default()
            .borders(Borders::ALL)
            .title(title_text)
            .border_style(Style::default().fg(Color::Cyan));
        render_pane(f, chunks[0], title_block);

        // Content: directory file list (left) + art preview (right)
        let content_area = chunks[1];
        let has_preview = art_picker.is_available() && content_area.width > 40;

        if has_preview {
            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(60),
                    Constraint::Percentage(40),
                ])
                .split(content_area);

            self.render_file_list(f, content_chunks[0]);
            self.render_art_preview(f, content_chunks[1], art_picker, art_cache);
        } else {
            self.render_file_list(f, content_area);
        }

        // Buttons
        let is_confirm = self.selected_button == AlbumArtReviewButton::Confirm;
        let buttons = [
            ConfirmationButton::new("Confirm", Color::Green).selected(is_confirm),
            ConfirmationButton::new("Skip", Color::Yellow).selected(!is_confirm),
        ];
        render_button_row(f, chunks[2], &buttons);

        // Hints
        let hints = Paragraph::new(Line::from(vec![
            cc::nav("[↑↓]"),
            cc::text(" scroll  "),
            cc::nav("[←→]"),
            cc::text(" buttons  "),
            cc::confirm("[Enter]"),
            cc::text(" confirm  "),
            cc::cancel("[Esc]"),
            cc::text(" cancel"),
        ]))
        .alignment(Alignment::Center);
        f.render_widget(hints, chunks[3]);
    }

    /// Render the file list for the current directory (left pane).
    fn render_file_list(&self, f: &mut Frame, area: Rect) {
        let Some(dir) = self.current() else {
            let block = Block::default().borders(Borders::ALL).title(" No directories ");
            f.render_widget(block, area);
            return;
        };

        let mut items: Vec<ListItem> = Vec::new();
        let mut line_idx: usize = 0;

        // Directory path as header
        let dir_title = format!(" {} ", dir.directory);

        // Embed section
        if !dir.embed_entries.is_empty() {
            let header_style = if line_idx == self.file_scroll {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  Embed ({} files):", dir.embed_entries.len()),
                header_style,
            ))));
            line_idx += 1;

            for entry in &dir.embed_entries {
                let filename = Path::new(&entry.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.path.clone());
                let style = if line_idx == self.file_scroll {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("    ♪ {}", filename),
                    style,
                ))));
                line_idx += 1;
            }
        }

        // Upgrade section
        if !dir.upgrade_entries.is_empty() {
            let header_style = if line_idx == self.file_scroll {
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  Upgrade ({} files):", dir.upgrade_entries.len()),
                header_style,
            ))));
            line_idx += 1;

            for entry in &dir.upgrade_entries {
                let filename = Path::new(&entry.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.path.clone());
                let style = if line_idx == self.file_scroll {
                    Style::default().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::default().fg(Color::White)
                };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("    ♪ {}", filename),
                    style,
                ))));
                line_idx += 1;

                // Show current art description below
                let desc = entry.current_art_desc.as_deref().unwrap_or("unknown");
                let desc_style = if line_idx == self.file_scroll {
                    Style::default().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("      ({} →)", desc),
                    desc_style,
                ))));
                line_idx += 1;
            }
        }

        // Apply scroll offset
        let visible_height = area.height.saturating_sub(2) as usize; // borders
        let skip = if self.file_scroll >= visible_height {
            self.file_scroll - visible_height + 1
        } else {
            0
        };
        let visible_items: Vec<ListItem> = items.into_iter().skip(skip).collect();

        let list = List::new(visible_items)
            .block(Block::default().borders(Borders::ALL).title(dir_title));
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
            .title(" Sidecar Preview ")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.width < 2 || inner.height < 2 {
            return;
        }

        if let Some(dir) = self.current() {
            let image_path = Path::new(&dir.sidecar.path);

            art_cache.retain_only(&[image_path]);
            let cached = art_cache.get_or_load(image_path, art_picker);

            let (preview_area, info_area) = if inner.height > 4 {
                let split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(inner);
                (split[0], Some(split[1]))
            } else {
                (inner, None)
            };

            render_album_art_preview(f, preview_area, cached);

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
