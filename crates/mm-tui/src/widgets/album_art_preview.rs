//! Album Art Preview Widget
//!
//! Renders album art images in the terminal using ratatui-image.
//! Automatically detects terminal graphics protocol (Kitty, Sixel, iTerm2)
//! with halfblock fallback.
//!
//! Usage:
//! - Call `AlbumArtPicker::init()` once after entering alternate screen
//! - Store the picker in app state
//! - Use `render_album_art_preview()` to render an image from a file path

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mm_meta::tags::{COVER_BACK_NAMES, COVER_FRONT_NAMES};

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::StatefulImage;

/// Wrapper around ratatui-image's Picker for terminal protocol detection.
///
/// Created once at app startup, used to create image protocols for rendering.
pub struct AlbumArtPicker {
    picker: Option<Picker>,
}

impl AlbumArtPicker {
    /// Initialize the picker using environment-based protocol detection.
    ///
    /// Uses `from_fontsize()` which detects kitty/iterm2/sixel from env vars
    /// without spawning threads or touching stdin. `from_query_stdio()` spawns
    /// a thread that does blocking reads on stdin with a 1s timeout — if the
    /// terminal is slow to respond (SSH, tmux), the thread outlives the timeout
    /// and competes with crossterm's event reader for stdin, stealing ~2/3 of
    /// keypresses. When it eventually finishes, it also restores pre-raw termios
    /// settings, clobbering crossterm's raw mode entirely.
    pub fn init() -> Self {
        // (10, 20) is ratatui-image's own default when font size can't be queried.
        // Exact font size only matters for pixel-perfect protocol rendering; for
        // halfblocks it's irrelevant, and for kitty/sixel it's close enough.
        let picker = Picker::from_fontsize((10, 20));
        Self {
            picker: Some(picker),
        }
    }

    /// Create a new stateful protocol for rendering an image.
    pub fn new_protocol(&mut self, img: image::DynamicImage) -> Option<StatefulProtocol> {
        self.picker.as_mut().map(|p| p.new_resize_protocol(img))
    }
}

/// Cached image state for a single art source.
///
/// Avoids re-reading and re-decoding the same image on every frame.
pub struct CachedArtProtocol {
    /// The path this cache entry was loaded from.
    pub path: PathBuf,
    /// The protocol state for rendering (None if image couldn't be loaded).
    pub protocol: Option<StatefulProtocol>,
    /// Image dimensions (for text fallback display).
    pub width: u32,
    pub height: u32,
    /// Format string (e.g., "png", "jpeg").
    pub format: String,
    /// Role of this image (e.g., "cover_front", "cover_back", "other").
    pub role: String,
}

/// Cache key distinguishing sidecar image files from embedded audio art.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum ArtCacheKey {
    /// Sidecar image file on disk (e.g., cover.jpg).
    Sidecar(PathBuf),
    /// Embedded art extracted from an audio file.
    Embedded(PathBuf),
}

/// Cache for album art image protocols, keyed by source.
///
/// Holds at most a few entries (one per visible context).
pub struct AlbumArtCache {
    entries: HashMap<ArtCacheKey, CachedArtProtocol>,
}

impl AlbumArtCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Load a sidecar image from disk and cache its protocol.
    /// Returns a mutable reference to the cached entry.
    pub fn get_or_load(
        &mut self,
        path: &Path,
        picker: &mut AlbumArtPicker,
    ) -> &mut CachedArtProtocol {
        let key = ArtCacheKey::Sidecar(path.to_path_buf());
        if !self.entries.contains_key(&key) {
            let entry = load_sidecar_image(path, picker);
            self.entries.insert(key.clone(), entry);
        }
        self.entries.get_mut(&key).unwrap()
    }

    /// Load embedded art from an audio file and cache its protocol.
    /// Returns a mutable reference to the cached entry.
    pub fn get_or_load_embedded(
        &mut self,
        audio_path: &Path,
        picker: &mut AlbumArtPicker,
    ) -> &mut CachedArtProtocol {
        let key = ArtCacheKey::Embedded(audio_path.to_path_buf());
        if !self.entries.contains_key(&key) {
            let entry = load_embedded_art(audio_path, picker);
            self.entries.insert(key.clone(), entry);
        }
        self.entries.get_mut(&key).unwrap()
    }

    /// Evict all entries whose key is not in the given set.
    /// Call this when selection changes to avoid unbounded growth.
    pub fn retain_only_keys(&mut self, keys: &[ArtCacheKey]) {
        self.entries.retain(|k, _| keys.contains(k));
    }
}

/// Load a sidecar image from disk and create a protocol for it.
fn load_sidecar_image(path: &Path, picker: &mut AlbumArtPicker) -> CachedArtProtocol {
    let result = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .and_then(|reader| {
            let format_hint = reader
                .format()
                .map(|f| format!("{:?}", f).to_lowercase())
                .unwrap_or_default();
            reader
                .decode()
                .map(|img| (img, format_hint))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        });

    // Derive role from filename stem
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let role = if COVER_FRONT_NAMES.iter().any(|&n| stem == n) {
        "cover_front"
    } else if COVER_BACK_NAMES.iter().any(|&n| stem == n) {
        "cover_back"
    } else {
        "other"
    }
    .to_string();

    match result {
        Ok((img, format)) => {
            let width = img.width();
            let height = img.height();
            let protocol = picker.new_protocol(img);
            CachedArtProtocol {
                path: path.to_path_buf(),
                protocol,
                width,
                height,
                format,
                role,
            }
        }
        Err(_) => CachedArtProtocol {
            path: path.to_path_buf(),
            protocol: None,
            width: 0,
            height: 0,
            format: String::new(),
            role,
        },
    }
}

/// Extract the first embedded picture from an audio file and create a protocol.
///
/// Prefers CoverFront pictures, falls back to the first picture found.
/// Uses lofty's generic Probe for format-agnostic extraction.
fn load_embedded_art(audio_path: &Path, picker: &mut AlbumArtPicker) -> CachedArtProtocol {
    let empty = CachedArtProtocol {
        path: audio_path.to_path_buf(),
        protocol: None,
        width: 0,
        height: 0,
        format: String::new(),
        role: String::new(),
    };

    // Open and read the audio file's tags
    let tagged_file = match lofty::probe::Probe::open(audio_path).and_then(|p| p.read()) {
        Ok(f) => f,
        Err(_) => return empty,
    };

    // Collect all pictures across all tags
    use lofty::file::TaggedFileExt;

    let mut best_picture: Option<&lofty::picture::Picture> = None;
    for tag in tagged_file.tags() {
        for pic in tag.pictures() {
            if pic.pic_type() == lofty::picture::PictureType::CoverFront {
                best_picture = Some(pic);
                break;
            }
            if best_picture.is_none() {
                best_picture = Some(pic);
            }
        }
        if best_picture.is_some_and(|p| p.pic_type() == lofty::picture::PictureType::CoverFront) {
            break;
        }
    }

    let picture = match best_picture {
        Some(p) => p,
        None => return empty,
    };

    // Decode the picture data into a DynamicImage
    let img = match image::load_from_memory(picture.data()) {
        Ok(img) => img,
        Err(_) => return empty,
    };

    // Extract dimensions, format, and role
    let width = img.width();
    let height = img.height();
    let format = match picture.mime_type() {
        Some(lofty::picture::MimeType::Jpeg) => "jpeg".to_string(),
        Some(lofty::picture::MimeType::Png) => "png".to_string(),
        Some(lofty::picture::MimeType::Bmp) => "bmp".to_string(),
        Some(lofty::picture::MimeType::Gif) => "gif".to_string(),
        Some(lofty::picture::MimeType::Tiff) => "tiff".to_string(),
        _ => "unknown".to_string(),
    };
    let role = match picture.pic_type() {
        lofty::picture::PictureType::CoverFront => "cover_front",
        lofty::picture::PictureType::CoverBack => "cover_back",
        _ => "other",
    }
    .to_string();

    let protocol = picker.new_protocol(img);

    CachedArtProtocol {
        path: audio_path.to_path_buf(),
        protocol,
        width,
        height,
        format,
        role,
    }
}

/// Render an album art preview in the given area.
///
/// If the image protocol is available, renders the image graphically.
/// Otherwise, renders a text fallback showing metadata.
pub fn render_album_art_preview(f: &mut Frame, area: Rect, cached: &mut CachedArtProtocol) {
    if area.width < 2 || area.height < 2 {
        return;
    }

    if let Some(ref mut protocol) = cached.protocol {
        let image_widget = StatefulImage::default();
        f.render_stateful_widget(image_widget, area, protocol);
    } else {
        render_text_fallback(f, area, cached);
    }
}

/// Text fallback when graphical rendering isn't available.
fn render_text_fallback(f: &mut Frame, area: Rect, cached: &CachedArtProtocol) {
    let filename = cached
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let info = if cached.width > 0 {
        format!(
            "[{} {}x{} {}]",
            filename,
            cached.width,
            cached.height,
            cached.format.to_uppercase()
        )
    } else {
        format!("[{}]", filename)
    };

    let line = Line::from(Span::styled(info, Style::default().fg(Color::DarkGray)));
    f.render_widget(Paragraph::new(line), area);
}

/// Render a "no album art" placeholder, centered both horizontally and vertically.
pub fn render_no_art_placeholder(f: &mut Frame, area: Rect) {
    if area.width < 2 || area.height < 1 {
        return;
    }
    let line = Line::from(Span::styled(
        "[no album art]",
        Style::default().fg(Color::DarkGray),
    ));
    // Center vertically by offsetting into the area
    let y_offset = area.height / 2;
    let centered_area = Rect {
        x: area.x,
        y: area.y + y_offset,
        width: area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(line).alignment(Alignment::Center),
        centered_area,
    );
}
