//! Album Art Review Modal
//!
//! Per-directory review of album art work: embedding sidecar images into artless
//! audio files and upgrading lower-quality embedded art with better sidecars.
//!
//! - Up/Down: Navigate file selection within current directory
//! - Tab/Shift-Tab: Cycle between directories (browse freely)
//! - Left/Right: Switch between Replace/Append/Skip buttons
//! - Enter: Replace, append, or skip current directory
//! - Escape: Cancel (with confirmation if directories have been confirmed)

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
use crate::meta::mutations::album_art::{AppendAlbumArtMutation, EmbedAlbumArtMutation, UpgradeAlbumArtMutation};
use crate::meta::signals::data::SidecarImage;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::control_colors as cc;
use crate::ui::widgets::{
    AlbumArtCache, AlbumArtPicker, ArtCacheKey, ConfirmationButton, ConfirmationModal,
    render_album_art_preview, render_button_row, render_no_art_placeholder,
    square_height,
};

// ============================================================================
// Actions
// ============================================================================

/// Actions returned from the album art review modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumArtReviewAction {
    /// No action needed.
    None,
    /// Replace current directory's upgrade entries (strip + re-embed).
    ReplaceDirectory,
    /// Append to current directory's upgrade entries (keep existing + add).
    AppendDirectory,
    /// Skip current directory without staging.
    SkipDirectory,
    /// Cancel entire review.
    Cancel,
}

// ============================================================================
// Data Types
// ============================================================================

/// Whether a file entry is an embed (artless) or upgrade operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtOperation {
    /// File has no embedded art — will embed sidecar.
    Embed,
    /// File has lower-quality art — will upgrade with sidecar.
    Upgrade,
}

/// A file entry in the flat file list, with its operation type.
pub struct FlatFileEntry<'a> {
    pub file: &'a ArtReviewFile,
    pub operation: ArtOperation,
}

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
    /// Generate mutations for this directory with the given button mode.
    ///
    /// Embed entries always produce `EmbedAlbumArt` regardless of mode.
    /// Upgrade entries produce `UpgradeAlbumArt` for Replace, `AppendAlbumArt` for Append.
    pub fn mutations_with_mode(&self, button: AlbumArtReviewButton) -> Vec<Mutation> {
        let resolver = paths::get_resolver();
        let image_path = PathBuf::from(&self.sidecar.path);
        let mut mutations = Vec::new();

        // Embed mutations — always the same regardless of mode
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

        // Upgrade mutations — mode-dependent
        if let Some(ref signal_key) = self.upgrade_signal_key {
            for entry in &self.upgrade_entries {
                let audio_path = resolver.resolve(Path::new(&entry.path));
                let current_art_desc = entry.current_art_desc.clone().unwrap_or_default();
                match button {
                    AlbumArtReviewButton::Replace => {
                        mutations.push(Mutation::UpgradeAlbumArt(UpgradeAlbumArtMutation {
                            inode: entry.inode,
                            audio_path,
                            image_path: image_path.clone(),
                            signal_key: signal_key.clone(),
                            current_art_desc,
                        }));
                    }
                    AlbumArtReviewButton::Append => {
                        mutations.push(Mutation::AppendAlbumArt(AppendAlbumArtMutation {
                            inode: entry.inode,
                            audio_path,
                            image_path: image_path.clone(),
                            signal_key: signal_key.clone(),
                            current_art_desc,
                        }));
                    }
                    AlbumArtReviewButton::Skip => {} // unreachable in practice
                }
            }
        }

        mutations
    }

    /// Return a flat list of all files (embed first, then upgrade) with operation tags.
    fn flat_files(&self) -> Vec<FlatFileEntry<'_>> {
        let mut entries = Vec::with_capacity(self.embed_entries.len() + self.upgrade_entries.len());
        for file in &self.embed_entries {
            entries.push(FlatFileEntry { file, operation: ArtOperation::Embed });
        }
        for file in &self.upgrade_entries {
            entries.push(FlatFileEntry { file, operation: ArtOperation::Upgrade });
        }
        entries
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
    Replace,
    Append,
    Skip,
}

/// State for the per-directory album art review modal.
#[derive(Debug)]
pub struct AlbumArtReviewState {
    /// All directories to review.
    pub directories: Vec<ArtReviewDirectory>,
    /// Index of current directory being viewed (browsable via Tab).
    pub current_dir: usize,
    /// Per-directory processed state (same length as `directories`).
    pub processed: Vec<bool>,
    /// Selected file index within the flat file list for the current directory.
    pub selected_file: usize,
    /// Which button is selected.
    pub selected_button: AlbumArtReviewButton,
    /// Accumulated mutations from confirmed directories.
    pub staged_mutations: Vec<Mutation>,
    /// Number of directories confirmed so far (not skipped).
    pub confirmed_count: usize,
    /// Whether the cancel confirmation popup is showing.
    pub show_cancel_confirm: bool,
    /// Which button is selected in the cancel confirmation popup.
    pub cancel_confirm_yes: bool,
}

impl AlbumArtReviewState {
    /// Create a new review state.
    pub fn new(directories: Vec<ArtReviewDirectory>) -> Self {
        let len = directories.len();
        Self {
            directories,
            current_dir: 0,
            processed: vec![false; len],
            selected_file: 0,
            selected_button: AlbumArtReviewButton::Replace,
            staged_mutations: Vec::new(),
            confirmed_count: 0,
            show_cancel_confirm: false,
            cancel_confirm_yes: false,
        }
    }

    /// Whether all directories have been processed (confirmed or skipped).
    pub fn is_complete(&self) -> bool {
        self.processed.iter().all(|&p| p)
    }

    /// Count of processed directories.
    fn processed_count(&self) -> usize {
        self.processed.iter().filter(|&&p| p).count()
    }

    /// Get the current directory, if any.
    fn current(&self) -> Option<&ArtReviewDirectory> {
        self.directories.get(self.current_dir)
    }

    /// Total file count in current directory's flat list.
    fn flat_file_count(&self) -> usize {
        self.current()
            .map(|d| d.embed_entries.len() + d.upgrade_entries.len())
            .unwrap_or(0)
    }

    /// Mark current directory as processed and navigate to the next unprocessed one.
    /// Returns true if all directories are now processed.
    pub fn mark_processed_and_advance(&mut self) -> bool {
        if self.current_dir < self.processed.len() {
            self.processed[self.current_dir] = true;
        }

        // Find next unprocessed directory
        if let Some(next) = self.next_unprocessed() {
            self.current_dir = next;
            self.selected_file = 0;
            self.selected_button = AlbumArtReviewButton::Replace;
        }

        self.is_complete()
    }

    /// Find the index of the next unprocessed directory (wrapping around).
    fn next_unprocessed(&self) -> Option<usize> {
        let len = self.directories.len();
        // Search forward from current_dir+1, wrapping
        for offset in 1..=len {
            let idx = (self.current_dir + offset) % len;
            if !self.processed[idx] {
                return Some(idx);
            }
        }
        None
    }

    /// Path of the currently selected directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.current().map(|d| d.directory.as_str())
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> AlbumArtReviewAction {
        // Cancel confirmation popup intercepts input
        if self.show_cancel_confirm {
            return self.handle_cancel_confirm_input(action);
        }

        match action {
            InputAction::Cancel => {
                if self.confirmed_count > 0 {
                    // Work would be lost — show confirmation popup
                    self.show_cancel_confirm = true;
                    self.cancel_confirm_yes = false;
                    AlbumArtReviewAction::None
                } else {
                    AlbumArtReviewAction::Cancel
                }
            }

            InputAction::Confirm => {
                match self.selected_button {
                    AlbumArtReviewButton::Replace => AlbumArtReviewAction::ReplaceDirectory,
                    AlbumArtReviewButton::Append => AlbumArtReviewAction::AppendDirectory,
                    AlbumArtReviewButton::Skip => AlbumArtReviewAction::SkipDirectory,
                }
            }

            InputAction::NavLeft => {
                self.selected_button = match self.selected_button {
                    AlbumArtReviewButton::Replace => AlbumArtReviewButton::Replace, // clamp
                    AlbumArtReviewButton::Append => AlbumArtReviewButton::Replace,
                    AlbumArtReviewButton::Skip => AlbumArtReviewButton::Append,
                };
                AlbumArtReviewAction::None
            }

            InputAction::NavRight => {
                self.selected_button = match self.selected_button {
                    AlbumArtReviewButton::Replace => AlbumArtReviewButton::Append,
                    AlbumArtReviewButton::Append => AlbumArtReviewButton::Skip,
                    AlbumArtReviewButton::Skip => AlbumArtReviewButton::Skip, // clamp
                };
                AlbumArtReviewAction::None
            }

            InputAction::NavUp => {
                if self.selected_file > 0 {
                    self.selected_file -= 1;
                }
                AlbumArtReviewAction::None
            }

            InputAction::NavDown => {
                let max = self.flat_file_count().saturating_sub(1);
                if self.selected_file < max {
                    self.selected_file += 1;
                }
                AlbumArtReviewAction::None
            }

            InputAction::Home => {
                self.selected_file = 0;
                AlbumArtReviewAction::None
            }

            InputAction::End => {
                self.selected_file = self.flat_file_count().saturating_sub(1);
                AlbumArtReviewAction::None
            }

            // Tab: browse to next directory (does NOT affect processed state)
            InputAction::CycleNext => {
                if self.directories.len() > 1 {
                    self.current_dir = (self.current_dir + 1) % self.directories.len();
                    self.selected_file = 0;
                }
                AlbumArtReviewAction::None
            }

            // Shift-Tab: browse to previous directory
            InputAction::CyclePrev => {
                if self.directories.len() > 1 {
                    self.current_dir = if self.current_dir == 0 {
                        self.directories.len() - 1
                    } else {
                        self.current_dir - 1
                    };
                    self.selected_file = 0;
                }
                AlbumArtReviewAction::None
            }

            _ => AlbumArtReviewAction::None,
        }
    }

    /// Handle input when the cancel confirmation popup is showing.
    fn handle_cancel_confirm_input(&mut self, action: &InputAction) -> AlbumArtReviewAction {
        match action {
            InputAction::Confirm => {
                if self.cancel_confirm_yes {
                    AlbumArtReviewAction::Cancel
                } else {
                    self.show_cancel_confirm = false;
                    AlbumArtReviewAction::None
                }
            }
            InputAction::Cancel => {
                self.show_cancel_confirm = false;
                AlbumArtReviewAction::None
            }
            InputAction::NavLeft | InputAction::NavRight => {
                self.cancel_confirm_yes = !self.cancel_confirm_yes;
                AlbumArtReviewAction::None
            }
            _ => AlbumArtReviewAction::None,
        }
    }

    /// Render the per-directory review modal.
    ///
    /// Returns `true` if a cache miss occurred during rendering (images were loaded
    /// from disk), signaling that the input buffer should be drained.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
    ) -> bool {
        let mut had_cache_miss = false;

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

        // Title — includes directory path and processed counter
        let dir_path = self.current()
            .map(|d| d.directory.as_str())
            .unwrap_or("");
        let is_current_processed = self.processed.get(self.current_dir).copied().unwrap_or(false);
        let processed_marker = if is_current_processed { " [done]" } else { "" };
        let title_text = format!(
            " Album Art Review [{}/{}]  {}{} ",
            self.processed_count(),
            self.directories.len(),
            dir_path,
            processed_marker,
        );
        let title_block = Block::default()
            .borders(Borders::ALL)
            .title(title_text)
            .border_style(Style::default().fg(Color::Cyan));
        render_pane(f, chunks[0], title_block);

        // Content: track list (left) + dual art preview (right)
        let content_area = chunks[1];
        let has_preview = art_picker.is_available() && content_area.width > 40;

        if has_preview {
            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(40),
                    Constraint::Percentage(60),
                ])
                .split(content_area);

            self.render_file_list(f, content_chunks[0]);
            had_cache_miss = self.render_dual_art_preview(f, content_chunks[1], art_picker, art_cache);
        } else {
            self.render_file_list(f, content_area);
        }

        // Buttons
        let buttons = [
            ConfirmationButton::new("Replace", Color::Green)
                .selected(self.selected_button == AlbumArtReviewButton::Replace),
            ConfirmationButton::new("Append", Color::Cyan)
                .selected(self.selected_button == AlbumArtReviewButton::Append),
            ConfirmationButton::new("Skip", Color::Yellow)
                .selected(self.selected_button == AlbumArtReviewButton::Skip),
        ];
        render_button_row(f, chunks[2], &buttons);

        // Hints
        let hints = Paragraph::new(Line::from(vec![
            cc::nav("[↑↓]"),
            cc::text(" files  "),
            cc::nav("[Tab/S-Tab]"),
            cc::text(" dir  "),
            cc::nav("[←→]"),
            cc::text(" buttons  "),
            cc::confirm("[Enter]"),
            cc::text(" confirm  "),
            cc::cancel("[Esc]"),
            cc::text(" cancel"),
        ]))
        .alignment(Alignment::Center);
        f.render_widget(hints, chunks[3]);

        // Cancel confirmation overlay
        if self.show_cancel_confirm {
            ConfirmationModal::new(" Discard Work? ")
                .border_color(Color::Yellow)
                .fixed_size(48, 8)
                .message(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("Discard {} confirmed director{}?",
                            self.confirmed_count,
                            if self.confirmed_count == 1 { "y" } else { "ies" },
                        ),
                        Style::default().fg(Color::Yellow),
                    )),
                ])
                .buttons(vec![
                    ConfirmationButton::new("No", Color::Green)
                        .selected(!self.cancel_confirm_yes),
                    ConfirmationButton::new("Yes, discard", Color::Red)
                        .selected(self.cancel_confirm_yes),
                ])
                .hint("←/→ switch  Enter confirm  Esc dismiss")
                .render(f, area);
        }

        had_cache_miss
    }

    /// Render the flat track list for the current directory (left pane).
    fn render_file_list(&self, f: &mut Frame, area: Rect) {
        let Some(dir) = self.current() else {
            let block = Block::default().borders(Borders::ALL).title(" No directories ");
            f.render_widget(block, area);
            return;
        };

        let flat = dir.flat_files();
        let mut items: Vec<ListItem> = Vec::with_capacity(flat.len());

        for (i, entry) in flat.iter().enumerate() {
            let filename = Path::new(&entry.file.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.file.path.clone());

            let is_selected = i == self.selected_file;
            let (note_color, cursor_bg) = match entry.operation {
                ArtOperation::Embed => (Color::Cyan, Color::Cyan),
                ArtOperation::Upgrade => (Color::Yellow, Color::Yellow),
            };

            let style = if is_selected {
                Style::default().fg(Color::Black).bg(cursor_bg)
            } else {
                Style::default().fg(Color::White)
            };

            let note_style = if is_selected {
                Style::default().fg(Color::Black).bg(cursor_bg)
            } else {
                Style::default().fg(note_color)
            };

            items.push(ListItem::new(Line::from(vec![
                Span::styled("  ", style),
                Span::styled("♪", note_style),
                Span::styled(format!(" {}", filename), style),
            ])));
        }

        // Viewport scrolling based on selected_file
        let visible_height = area.height.saturating_sub(2) as usize; // borders
        let skip = if visible_height > 0 && self.selected_file >= visible_height {
            self.selected_file - visible_height + 1
        } else {
            0
        };
        let visible_items: Vec<ListItem> = items.into_iter().skip(skip).collect();

        let list = List::new(visible_items)
            .block(Block::default().borders(Borders::ALL).title(" Tracks "));
        f.render_widget(list, area);
    }

    /// Render dual art preview: current (top) + new/sidecar (bottom).
    ///
    /// Returns `true` if a cache miss occurred (image loaded from disk).
    fn render_dual_art_preview(
        &self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
    ) -> bool {
        let mut had_cache_miss = false;

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.width < 4 || inner.height < 4 {
            return false;
        }

        let Some(dir) = self.current() else {
            render_no_art_placeholder(f, inner);
            return false;
        };

        let flat = dir.flat_files();
        let selected = flat.get(self.selected_file);

        // Determine cache keys to retain
        let sidecar_path = Path::new(&dir.sidecar.path);
        let sidecar_key = ArtCacheKey::Sidecar(sidecar_path.to_path_buf());

        let mut retain_keys = vec![sidecar_key.clone()];
        if let Some(entry) = &selected {
            if entry.operation == ArtOperation::Upgrade {
                let resolver = paths::get_resolver();
                let audio_path = resolver.resolve(Path::new(&entry.file.path));
                retain_keys.push(ArtCacheKey::Embedded(audio_path));
            }
        }
        art_cache.retain_only_keys(&retain_keys);

        let font_size = art_picker.font_size();

        // Each art section: 1 line label + square art area
        // We have two sections: "Current" and "New"
        // Compute ideal square height for the available width
        let ideal_sq = square_height(inner.width, font_size);

        // Available height: inner.height, split between two label+art pairs
        // Each pair needs 1 (label) + art_height
        // Total: 2 labels + 2 art areas = 2 + 2*art_h <= inner.height
        let available_for_art = inner.height.saturating_sub(2); // 2 label lines
        let art_h = if ideal_sq * 2 <= available_for_art {
            ideal_sq
        } else {
            available_for_art / 2
        };

        if art_h < 1 {
            return false;
        }

        // Layout: current_label, current_art, new_label, new_art
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),    // "Current" label
                Constraint::Length(art_h), // current art
                Constraint::Length(1),    // "New" label
                Constraint::Length(art_h), // new art
                Constraint::Min(0),       // any remaining space
            ])
            .split(inner);

        // === Current art (top) ===
        if let Some(entry) = &selected {
            match entry.operation {
                ArtOperation::Embed => {
                    // Artless file — show "Current" label + placeholder
                    let label = Line::from(Span::styled(
                        " Current",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ));
                    f.render_widget(Paragraph::new(label), sections[0]);
                    render_no_art_placeholder(f, sections[1]);
                }
                ArtOperation::Upgrade => {
                    // Has embedded art — load and show it
                    let resolver = paths::get_resolver();
                    let audio_path = resolver.resolve(Path::new(&entry.file.path));

                    // Detect cache miss before loading
                    let key = ArtCacheKey::Embedded(audio_path.clone());
                    if !art_cache.has_key(&key) {
                        had_cache_miss = true;
                    }

                    let cached = art_cache.get_or_load_embedded(&audio_path, art_picker);

                    let label_text = if cached.width > 0 {
                        format!(
                            " Current ({}x{} {})",
                            cached.width,
                            cached.height,
                            cached.format.to_uppercase(),
                        )
                    } else {
                        " Current".to_string()
                    };
                    let label = Line::from(Span::styled(
                        label_text,
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ));
                    f.render_widget(Paragraph::new(label), sections[0]);
                    render_album_art_preview(f, sections[1], cached);
                }
            }
        } else {
            let label = Line::from(Span::styled(
                " Current",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
            ));
            f.render_widget(Paragraph::new(label), sections[0]);
            render_no_art_placeholder(f, sections[1]);
        }

        // === New art (bottom) ===
        {
            // Detect cache miss before loading
            if !art_cache.has_key(&sidecar_key) {
                had_cache_miss = true;
            }

            let cached = art_cache.get_or_load(sidecar_path, art_picker);
            let label_text = if cached.width > 0 {
                format!(
                    " New ({}x{} {})",
                    cached.width,
                    cached.height,
                    cached.format.to_uppercase(),
                )
            } else {
                " New".to_string()
            };
            let label = Line::from(Span::styled(
                label_text,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
            f.render_widget(Paragraph::new(label), sections[2]);
            render_album_art_preview(f, sections[3], cached);
        }

        had_cache_miss
    }
}
