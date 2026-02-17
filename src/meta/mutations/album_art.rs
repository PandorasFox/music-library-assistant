//! Embed Album Art Mutation
//!
//! Embeds a sidecar image file into an audio file as an embedded picture.
//! Supports FLAC (via lofty FlacFile::insert_picture) and Opus/OGG
//! (via VorbisComments METADATA_BLOCK_PICTURE).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::meta::recomputation::RecomputationScope;
use crate::meta::signals::data::EmbeddableAlbumArtSignal;
use crate::witch::MutationExecutionWitness;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{DiffEntry, Mutation, MutationResult, SignalClearScope, SignalToClear, path_filename};

/// Embed a sidecar image into an audio file.
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

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();

        let result = embed_picture(&self.audio_path, &self.image_path, ctx.witness);

        let (success, error) = match result {
            Ok(()) => {
                // Update DB: mark has_pictures = 1 and sync mtime/size.
                // This covers both actual embeds and no-ops (file already had art
                // but has_pictures was 0 from migration default).
                if let Some(sender) = crate::db_thread::signal_sender() {
                    let meta = std::fs::metadata(&self.audio_path).ok();
                    let (mtime_secs, mtime_nanos, file_size) = meta
                        .map(|m| {
                            use std::os::unix::fs::MetadataExt;
                            (m.mtime(), m.mtime_nsec(), m.size() as i64)
                        })
                        .unwrap_or((0, 0, 0));
                    sender.set_has_pictures(
                        self.inode,
                        mtime_secs,
                        mtime_nanos,
                        file_size,
                        ctx.witness,
                    );
                }
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

    // Read image data
    let image_data = std::fs::read(image_path)
        .with_context(|| format!("Failed to read image: {}", image_path.display()))?;

    // Determine MIME type from extension
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

    let picture = lofty::picture::Picture::unchecked(image_data)
        .pic_type(lofty::picture::PictureType::CoverFront)
        .mime_type(mime_type)
        .build();

    let audio_ext = audio_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    match audio_ext.as_str() {
        "flac" => embed_picture_flac(audio_path, picture),
        "opus" | "ogg" => embed_picture_vorbis(audio_path, picture),
        other => Err(anyhow::anyhow!(
            "Unsupported format for picture embedding: {}",
            other
        )),
    }
}

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

    // Get the VorbisComments tag, or create one
    let mut tag = tagged_file
        .tag(TagType::VorbisComments)
        .cloned()
        .unwrap_or_else(|| lofty::tag::Tag::new(TagType::VorbisComments));

    tag.push_picture(picture);
    tag.save_to_path(path, WriteOptions::new())
        .with_context(|| format!("Failed to save picture to {}", path.display()))?;

    Ok(())
}
