//! Album Art Mutations
//!
//! Embeds sidecar image files into audio files as embedded pictures, or replaces
//! existing embedded art with better sidecar images.
//!
//! Supports FLAC (via lofty FlacFile::insert_picture), Opus/OGG
//! (via VorbisComments METADATA_BLOCK_PICTURE), and MP3 (via ID3v2).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::meta::recomputation::RecomputationScope;
use crate::meta::signals::data::{EmbeddableAlbumArtSignal, UpgradeableAlbumArtSignal};
use crate::witch::MutationExecutionWitness;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{DiffEntry, Mutation, MutationResult, SignalClearScope, SignalToClear, path_filename};

/// Embed a sidecar image into an audio file that lacks embedded art.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedAlbumArtMutation {
    pub inode: i64,
    pub audio_path: PathBuf,
    pub image_path: PathBuf,
    /// Signal key (relative directory path) for clearing after batch.
    pub signal_key: String,
}

impl MutationExecutor for EmbedAlbumArtMutation {
    fn label(&self) -> &'static str {
        "Embed album art"
    }
    fn staging(&self) -> super::traits::MutationStaging { super::traits::MutationStaging::Staged(super::traits::MutationExecutionStage::DiskFlush) }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();

        let result = embed_picture(&self.audio_path, &self.image_path, ctx.witness);

        let (success, error) = match result {
            Ok(()) => {
                update_db_picture_state(self.inode, &self.audio_path, ctx.witness);
                (true, None)
            }
            Err(e) => (false, Some(format!("{:#}", e))),
        };

        MutationResult {
            _mutation: Mutation::EmbedAlbumArt(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }

    fn affected_inodes(&self) -> Vec<i64> {
        vec![self.inode]
    }

    fn recomputation_scope(&self) -> RecomputationScope { RecomputationScope::FILES }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        vec![SignalToClear::exact::<EmbeddableAlbumArtSignal>(self.signal_key.clone())]
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.audio_path.clone()]
    }

    fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            path_filename(&self.audio_path),
            "[no embedded art]",
            path_filename(&self.image_path),
        )]
    }
}

/// Append a sidecar image to an audio file without removing existing art.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendAlbumArtMutation {
    pub inode: i64,
    pub audio_path: PathBuf,
    pub image_path: PathBuf,
    /// Signal key for clearing after batch.
    pub signal_key: String,
    /// Description of current embedded art for diff display.
    pub current_art_desc: String,
}

impl MutationExecutor for AppendAlbumArtMutation {
    fn label(&self) -> &'static str {
        "Append album art"
    }
    fn staging(&self) -> super::traits::MutationStaging { super::traits::MutationStaging::Staged(super::traits::MutationExecutionStage::DiskFlush) }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();

        let result = append_picture(&self.audio_path, &self.image_path, ctx.witness);

        let (success, error) = match result {
            Ok(()) => {
                update_db_picture_state(self.inode, &self.audio_path, ctx.witness);
                (true, None)
            }
            Err(e) => (false, Some(format!("{:#}", e))),
        };

        MutationResult {
            _mutation: Mutation::AppendAlbumArt(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }

    fn affected_inodes(&self) -> Vec<i64> {
        vec![self.inode]
    }

    fn recomputation_scope(&self) -> RecomputationScope { RecomputationScope::FILES }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        vec![SignalToClear::exact::<UpgradeableAlbumArtSignal>(self.signal_key.clone())]
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.audio_path.clone()]
    }

    fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            path_filename(&self.audio_path),
            format!("{} (kept)", self.current_art_desc),
            format!("+ {}", path_filename(&self.image_path)),
        )]
    }
}

/// Replace existing embedded art with a better sidecar image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpgradeAlbumArtMutation {
    pub inode: i64,
    pub audio_path: PathBuf,
    pub image_path: PathBuf,
    /// Signal key for clearing after batch.
    pub signal_key: String,
    /// Description of current embedded art for diff display.
    pub current_art_desc: String,
}

impl MutationExecutor for UpgradeAlbumArtMutation {
    fn label(&self) -> &'static str {
        "Upgrade album art"
    }
    fn staging(&self) -> super::traits::MutationStaging { super::traits::MutationStaging::Staged(super::traits::MutationExecutionStage::DiskFlush) }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();

        let result = replace_picture(&self.audio_path, &self.image_path, ctx.witness);

        let (success, error) = match result {
            Ok(()) => {
                update_db_picture_state(self.inode, &self.audio_path, ctx.witness);
                (true, None)
            }
            Err(e) => (false, Some(format!("{:#}", e))),
        };

        MutationResult {
            _mutation: Mutation::UpgradeAlbumArt(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }

    fn affected_inodes(&self) -> Vec<i64> {
        vec![self.inode]
    }

    fn recomputation_scope(&self) -> RecomputationScope { RecomputationScope::FILES }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        vec![SignalToClear::exact::<UpgradeableAlbumArtSignal>(self.signal_key.clone())]
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.audio_path.clone()]
    }

    fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            path_filename(&self.audio_path),
            &self.current_art_desc,
            path_filename(&self.image_path),
        )]
    }
}

/// Update DB picture state after an embed or upgrade operation.
fn update_db_picture_state(inode: i64, audio_path: &std::path::Path, witness: &MutationExecutionWitness) {
    if let Some(sender) = crate::db::write_thread::signal_sender() {
        let meta = std::fs::metadata(audio_path).ok();
        let (mtime_secs, mtime_nanos, file_size) = meta
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                (m.mtime(), m.mtime_nsec(), m.size() as i64)
            })
            .unwrap_or((0, 0, 0));
        let pic_info = crate::corpus::tags::TagSet::extract_picture_info(audio_path);
        sender.set_has_pictures(
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
            pic_info.as_ref().map(|p| p.format.clone()),
            pic_info.as_ref().map(|p| p.width),
            pic_info.as_ref().map(|p| p.height),
            pic_info.as_ref().map(|p| p.count).unwrap_or(0),
            witness,
        );
    }
}

/// Read an image from disk and embed it into an audio file.
/// No-ops if the file already has embedded pictures.
fn embed_picture(
    audio_path: &std::path::Path,
    image_path: &std::path::Path,
    _witness: &MutationExecutionWitness,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use crate::corpus::tags::TagSet;

    // Guard: skip files that already have art
    if TagSet::has_embedded_pictures(audio_path) {
        return Ok(());
    }

    let picture = read_image_as_picture(image_path)?;
    let audio_ext = audio_ext(audio_path);

    match audio_ext.as_str() {
        "flac" => embed_picture_flac(audio_path, picture),
        "opus" | "ogg" => embed_picture_vorbis(audio_path, picture),
        "mp3" => embed_picture_mp3(audio_path, picture),
        other => Err(anyhow::anyhow!(
            "Unsupported format for picture embedding: {}",
            other
        )),
    }
    .with_context(|| format!("Failed to embed picture into {}", audio_path.display()))
}

/// Read an image from disk, strip existing art, and embed the new image.
fn replace_picture(
    audio_path: &std::path::Path,
    image_path: &std::path::Path,
    _witness: &MutationExecutionWitness,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let picture = read_image_as_picture(image_path)?;
    let audio_ext = audio_ext(audio_path);

    match audio_ext.as_str() {
        "flac" => replace_picture_flac(audio_path, picture),
        "opus" | "ogg" => replace_picture_vorbis(audio_path, picture),
        "mp3" => replace_picture_mp3(audio_path, picture),
        other => Err(anyhow::anyhow!(
            "Unsupported format for picture replacement: {}",
            other
        )),
    }
    .with_context(|| format!("Failed to replace picture in {}", audio_path.display()))
}

/// Read an image file and build a lofty Picture (CoverFront).
fn read_image_as_picture(
    image_path: &std::path::Path,
) -> anyhow::Result<lofty::picture::Picture> {
    use anyhow::Context;

    let image_data = std::fs::read(image_path)
        .with_context(|| format!("Failed to read image: {}", image_path.display()))?;

    let mime_type = match image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => lofty::picture::MimeType::Jpeg,
        Some("png") => lofty::picture::MimeType::Png,
        Some("gif") => lofty::picture::MimeType::Gif,
        Some("bmp") => lofty::picture::MimeType::Bmp,
        _ => lofty::picture::MimeType::Jpeg, // fallback
    };

    Ok(lofty::picture::Picture::unchecked(image_data)
        .pic_type(lofty::picture::PictureType::CoverFront)
        .mime_type(mime_type)
        .build())
}

/// Get the lowercase extension of an audio file.
fn audio_ext(path: &std::path::Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}

// =============================================================================
// Embed functions (add art to artless files)
// =============================================================================

/// Embed picture into a FLAC file via lofty's OggPictureStorage.
///
/// Known limitation (lofty 0.23.x): FLAC files with a prepended ID3v2 header
/// will fail with "File missing fLaC stream marker". lofty's `read_from()`
/// correctly skips the ID3v2, but `save_to_path()` re-reads the file from disk
/// and expects "fLaC" at byte 0. `remove_id3v2()` only strips the in-memory
/// model. These files must be fixed manually (strip the ID3v2 prefix) before
/// embedding will work. See `repro_lofty_flac_id3v2.rs` for upstream repro.
fn embed_picture_flac(
    path: &std::path::Path,
    picture: lofty::picture::Picture,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::ogg::OggPictureStorage;

    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open FLAC: {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default())
        .with_context(|| format!("Failed to read FLAC: {}", path.display()))?;
    drop(reader);

    // Strip ID3v2 from in-memory model (harmless no-op if absent)
    flac.remove_id3v2();

    // info=None lets lofty infer PictureInformation from the picture data
    flac.insert_picture(picture, None)
        .with_context(|| "Failed to insert picture into FLAC")?;

    flac.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save FLAC with picture: {}", path.display()))?;

    Ok(())
}

/// Embed picture into an Opus/OGG file via VorbisComments.
fn embed_picture_vorbis(
    path: &std::path::Path,
    picture: lofty::picture::Picture,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{TagExt, TagType};

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open for picture embedding: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read for picture embedding: {}", path.display()))?;

    let mut tag = tagged_file
        .tag(TagType::VorbisComments)
        .cloned()
        .unwrap_or_else(|| lofty::tag::Tag::new(TagType::VorbisComments));

    tag.push_picture(picture);
    tag.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save picture to {}", path.display()))?;

    Ok(())
}

/// Embed picture into an MP3 file via ID3v2.
fn embed_picture_mp3(
    path: &std::path::Path,
    picture: lofty::picture::Picture,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{TagExt, TagType};

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open MP3 for picture embedding: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read MP3 for picture embedding: {}", path.display()))?;

    let mut tag = tagged_file
        .tag(TagType::Id3v2)
        .cloned()
        .unwrap_or_else(|| lofty::tag::Tag::new(TagType::Id3v2));

    tag.push_picture(picture);
    tag.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save picture to MP3: {}", path.display()))?;

    Ok(())
}

// =============================================================================
// Replace functions (strip existing art, then embed new)
// =============================================================================

/// Replace pictures in a FLAC file.
fn replace_picture_flac(
    path: &std::path::Path,
    picture: lofty::picture::Picture,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::ogg::OggPictureStorage;

    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open FLAC: {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default())
        .with_context(|| format!("Failed to read FLAC: {}", path.display()))?;
    drop(reader);

    flac.remove_id3v2();

    // Remove all existing pictures
    while !flac.pictures().is_empty() {
        flac.remove_picture(0);
    }
    // Also clear VorbisComments pictures
    if let Some(vc) = flac.vorbis_comments_mut() {
        use lofty::ogg::OggPictureStorage;
        while !vc.pictures().is_empty() {
            vc.remove_picture(0);
        }
    }

    flac.insert_picture(picture, None)
        .with_context(|| "Failed to insert replacement picture into FLAC")?;

    flac.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save FLAC with replaced picture: {}", path.display()))?;

    Ok(())
}

/// Replace pictures in an Opus/OGG file.
fn replace_picture_vorbis(
    path: &std::path::Path,
    picture: lofty::picture::Picture,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{TagExt, TagType};

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open for picture replacement: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read for picture replacement: {}", path.display()))?;

    let mut tag = tagged_file
        .tag(TagType::VorbisComments)
        .cloned()
        .unwrap_or_else(|| lofty::tag::Tag::new(TagType::VorbisComments));

    // Remove existing pictures
    tag.remove_picture_type(lofty::picture::PictureType::CoverFront);
    tag.remove_picture_type(lofty::picture::PictureType::CoverBack);
    tag.remove_picture_type(lofty::picture::PictureType::Other);

    tag.push_picture(picture);
    tag.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save replaced picture to {}", path.display()))?;

    Ok(())
}

/// Replace pictures in an MP3 file.
fn replace_picture_mp3(
    path: &std::path::Path,
    picture: lofty::picture::Picture,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{TagExt, TagType};

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open MP3 for picture replacement: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read MP3 for picture replacement: {}", path.display()))?;

    let mut tag = tagged_file
        .tag(TagType::Id3v2)
        .cloned()
        .unwrap_or_else(|| lofty::tag::Tag::new(TagType::Id3v2));

    tag.remove_picture_type(lofty::picture::PictureType::CoverFront);
    tag.remove_picture_type(lofty::picture::PictureType::CoverBack);
    tag.remove_picture_type(lofty::picture::PictureType::Other);

    tag.push_picture(picture);
    tag.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save replaced picture to MP3: {}", path.display()))?;

    Ok(())
}

// =============================================================================
// Append functions (add art alongside existing pictures)
// =============================================================================

/// Read an image from disk and append it to an audio file's existing pictures.
/// Unlike `embed_picture`, this does NOT guard against existing art — it always adds.
/// Unlike `replace_picture`, this does NOT strip existing art — it keeps everything.
fn append_picture(
    audio_path: &std::path::Path,
    image_path: &std::path::Path,
    _witness: &MutationExecutionWitness,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let picture = read_image_as_picture(image_path)?;
    let audio_ext = audio_ext(audio_path);

    match audio_ext.as_str() {
        "flac" => embed_picture_flac(audio_path, picture),
        "opus" | "ogg" => embed_picture_vorbis(audio_path, picture),
        "mp3" => embed_picture_mp3(audio_path, picture),
        other => Err(anyhow::anyhow!(
            "Unsupported format for picture append: {}",
            other
        )),
    }
    .with_context(|| format!("Failed to append picture to {}", audio_path.display()))
}
