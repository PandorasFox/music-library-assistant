//! Tag Operations Module
//!
//! THIS IS THE ONLY MODULE THAT TOUCHES AUDIO FILE TAGS.
//!
//! All tag reading, writing, and comparison goes through this module.
//! Do not use lofty directly elsewhere. Do not duplicate this logic.
//! If you need tag functionality not provided here, ADD IT HERE.
//!
//! ## Architecture
//!
//! Tags are modeled as `TagSet`: a set of (key, value) tuples.
//! This naturally handles multi-value fields (multiple genres, etc).
//!
//! ## Entry Points
//!
//! - [`from_file()`] - Read all tags from audio file
//! - [`write_file_tags()`] - Write complete tag set to file
//! - [`TagSet::diff()`] - Compare two tag sets
//!
//! ## DO NOT
//!
//! - Import lofty in other modules
//! - Create "convenience" wrappers elsewhere
//! - Duplicate any of this logic

use anyhow::{Context, Result};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::ogg::OggPictureStorage;
use std::path::Path;

use crate::db::write_thread;
use crate::meta::mutations::MutationToken;
use crate::witch::MutationExecutionWitness;

// Re-export data types from mm-meta
pub use mm_meta::tags::{PictureInfo, TagSet};

/// Extract lowercase file extension from a path.
fn path_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}

/// Open a file for buffered reading with context on failure.
fn open_buffered(path: &Path) -> Result<std::io::BufReader<std::fs::File>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;
    Ok(std::io::BufReader::new(file))
}

// =============================================================================
// File I/O — Tag Reading
// =============================================================================

/// Read tags from an audio file.
    ///
/// For Vorbis-format files (FLAC, Opus, OGG Vorbis), reads directly from
/// VorbisComments — raw key=value pairs with exact key names, no ItemKey mapping.
/// This eliminates the non-bijective ItemKey::Unknown round-trip problem.
///
/// For other formats (mp3, m4a, etc.), falls back to lofty's generic Tag/Probe.
/// Binary tags (album art, etc.) are skipped.
pub fn from_file(path: &Path) -> Result<TagSet> {
    match path_ext(path).as_str() {
        "flac" => {
            let mut reader = open_buffered(path)?;
            let flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default())
                .with_context(|| format!("Failed to read tags from: {}", path.display()))?;
            Ok(tagset_from_vorbis_comments(flac.vorbis_comments()))
        }
        "opus" => {
            let mut reader = open_buffered(path)?;
            let opus = lofty::ogg::OpusFile::read_from(&mut reader, ParseOptions::default())
                .with_context(|| format!("Failed to read tags from: {}", path.display()))?;
            Ok(tagset_from_vorbis_comments(Some(opus.vorbis_comments())))
        }
        "ogg" => {
            let mut reader = open_buffered(path)?;
            let vorbis =
                lofty::ogg::VorbisFile::read_from(&mut reader, ParseOptions::default())
                    .with_context(|| format!("Failed to read tags from: {}", path.display()))?;
            Ok(tagset_from_vorbis_comments(Some(vorbis.vorbis_comments())))
        }
        _ => tagset_from_generic_tag(path),
    }
}

/// Extract metadata about embedded pictures from an audio file.
///
/// Returns info for the first CoverFront picture found (or first picture if
/// no CoverFront). Resolution is extracted via `PictureInformation` (PNG/JPEG
/// only; zeroed for other formats).
///
/// Non-fatal — returns None on read errors.
pub fn extract_picture_info(path: &Path) -> Option<PictureInfo> {
    match path_ext(path).as_str() {
        "flac" => {
            let file = std::fs::File::open(path).ok()?;
            let mut reader = std::io::BufReader::new(file);
            let flac =
                lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default()).ok()?;

            let mut all_pics: Vec<&(
                lofty::picture::Picture,
                lofty::picture::PictureInformation,
            )> = flac.pictures().iter().collect();
            if let Some(vc) = flac.vorbis_comments() {
                all_pics.extend(vc.pictures().iter());
            }
            pick_picture_info(&all_pics)
        }
        "opus" => {
            let file = std::fs::File::open(path).ok()?;
            let mut reader = std::io::BufReader::new(file);
            let opus =
                lofty::ogg::OpusFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            pick_picture_info(opus.vorbis_comments().pictures())
        }
        "ogg" => {
            let file = std::fs::File::open(path).ok()?;
            let mut reader = std::io::BufReader::new(file);
            let vorbis =
                lofty::ogg::VorbisFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            pick_picture_info(vorbis.vorbis_comments().pictures())
        }
        _ => {
            use lofty::file::TaggedFileExt;
            use lofty::picture::PictureInformation;
            use lofty::probe::Probe;

            let tagged_file = Probe::open(path).ok().and_then(|p| p.read().ok())?;
            let all_pictures: Vec<&lofty::picture::Picture> =
                tagged_file.tags().iter().flat_map(|t| t.pictures()).collect();

            if all_pictures.is_empty() {
                return None;
            }

            let count = all_pictures.len() as u32;
            let pic = all_pictures
                .iter()
                .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
                .or_else(|| all_pictures.first())
                .unwrap();

            let info = PictureInformation::from_picture(pic).unwrap_or_default();
            Some(PictureInfo {
                format: mime_type_to_format(pic.mime_type()),
                width: info.width,
                height: info.height,
                count,
            })
        }
    }
}

/// Build a TagSet from VorbisComments (shared by FLAC, Opus, OGG Vorbis).
///
/// Reads raw key=value pairs directly — no ItemKey mapping, no bijection problem.
fn tagset_from_vorbis_comments(vc: Option<&lofty::ogg::VorbisComments>) -> TagSet {
    let Some(vc) = vc else {
        return TagSet::empty();
    };
    let tags: Vec<(String, String)> = vc
        .items()
        .filter(|(k, _)| !is_binary_tag_key(k))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    TagSet::new(tags)
}

/// Fallback tag reading for non-Vorbis formats (mp3, m4a, etc.).
///
/// Uses lofty's generic Probe/Tag items(), mapping keys to canonical Vorbis names
/// via `ItemKey::map_key(TagType::VorbisComments)`.
/// These formats are read-only (pre-transcode initial indexing) and never written to.
fn tagset_from_generic_tag(path: &Path) -> Result<TagSet> {
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::TagType;

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag reading: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    let mut all_tags = Vec::new();

    if let Some(tag) = tagged_file.primary_tag() {
        let tag_type = tag.tag_type();

        for item in tag.items() {
            // Canonical Vorbis name is primary (ARTIST, TRACKNUMBER, etc.)
            // Fall back to source format's native name for tags without Vorbis mapping
            let key = match item.key().map_key(TagType::VorbisComments) {
                Some(k) => k.to_string(),
                None => match item.key().map_key(tag_type) {
                    Some(k) => k.to_string(),
                    None => continue,
                },
            };

            if is_binary_tag_key(&key) {
                continue;
            }

            let value = match item.value() {
                lofty::tag::ItemValue::Text(s) => s.clone(),
                lofty::tag::ItemValue::Locator(s) => s.clone(),
                lofty::tag::ItemValue::Binary(_) => continue,
            };

            if !value.is_empty() {
                all_tags.push((key, value));
            }
        }
    }

    Ok(TagSet::new(all_tags))
}

// =============================================================================
// Write Function - THE ONLY WAY TO WRITE TAGS
// =============================================================================

/// Write a complete TagSet to an audio file (pure file I/O, no DB side effects).
///
/// Replaces all text tags in the file with the provided set using format-specific
/// concrete types (VorbisComments for FLAC/Opus/OGG).
///
/// # Multi-value Support
///
/// Each (key, value) pair becomes a separate VorbisComments entry, naturally
/// supporting multiple values per key (e.g., multiple genres).
///
/// Used by both `write_file_tags()` (mutation context) and `copy_tags()` (transcode).
pub(crate) fn write_tags_to_file(path: &Path, tags: &TagSet) -> Result<()> {
    match path_ext(path).as_str() {
        "flac" => write_vorbis_tags_flac(path, tags),
        "opus" => write_vorbis_tags_opus(path, tags),
        "ogg" => write_vorbis_tags_ogg(path, tags),
        other => Err(anyhow::anyhow!("Cannot write tags to .{} format", other)),
    }
}

/// Write a complete TagSet to an audio file, with pending_write marker.
///
/// THIS IS THE ONLY FUNCTION THAT WRITES TAGS TO FILES FROM MUTATION CONTEXT.
///
/// Marks the inode as `pending_write` in `dirty_inodes` before writing, so
/// that when the FS watcher detects the change and VerifyTags runs, it can
/// distinguish MM-initiated writes from external changes.
///
/// All post-write reconciliation (mtime update, signal clearing) is handled
/// by VerifyTags via the watcher path — this function only writes tags.
///
/// # Authorization
///
/// Requires `MutationToken` proving this is called from mutation context.
/// Requires `MutationExecutionWitness` to authorize DB updates.
/// Do not call from UI code or computations.
pub fn write_file_tags(
    path: &Path,
    tags: &TagSet,
    _token: &MutationToken,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    // Mark pending_write before disk write so VerifyTags knows this is ours.
    // Fire-and-forget to db_thread — wait_for_queue_drain() in VerifyTags
    // guarantees this row is committed before it checks.
    let sender = write_thread::require_sender()?;
    let file_metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata before write: {}", path.display()))?;
    use std::os::unix::fs::MetadataExt;
    let inode = file_metadata.ino() as i64;
    sender.mark_dirty_inodes(vec![inode], "pending_write", witness);

    write_tags_to_file(path, tags)?;

    Ok(())
}

// =============================================================================
// Internal Helpers - Binary tag filtering and format-specific writers
// =============================================================================

/// Pick the best picture from a list of (Picture, PictureInformation) pairs.
///
/// Prefers CoverFront, falls back to first picture. Returns None if empty.
fn pick_picture_info<P: std::borrow::Borrow<(lofty::picture::Picture, lofty::picture::PictureInformation)>>(
    pics: &[P],
) -> Option<PictureInfo> {
    if pics.is_empty() {
        return None;
    }
    let count = pics.len() as u32;
    let pair = pics
        .iter()
        .find(|p| p.borrow().0.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| pics.first())
        .unwrap()
        .borrow();
    Some(PictureInfo {
        format: mime_type_to_format(pair.0.mime_type()),
        width: pair.1.width,
        height: pair.1.height,
        count,
    })
}

/// Convert a lofty MimeType to a short format string for DB storage.
fn mime_type_to_format(mime: Option<&lofty::picture::MimeType>) -> String {
    match mime {
        Some(lofty::picture::MimeType::Jpeg) => "jpeg".to_string(),
        Some(lofty::picture::MimeType::Png) => "png".to_string(),
        Some(lofty::picture::MimeType::Gif) => "gif".to_string(),
        Some(lofty::picture::MimeType::Bmp) => "bmp".to_string(),
        Some(lofty::picture::MimeType::Tiff) => "tiff".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Extract image dimensions from an image file on disk.
///
/// Uses the `image` crate's header-only reader — reads only enough bytes
/// to determine dimensions without decoding the full image.
/// Returns (width, height, format_string). Returns (0, 0, format) if dimensions
/// can't be determined.
pub fn image_dimensions(path: &Path) -> (u32, u32, String) {
    let ext = path_ext(path);
    let format = match ext.as_str() {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        "gif" => "gif",
        "bmp" => "bmp",
        "webp" => "webp",
        "tiff" | "tif" => "tiff",
        _ => "unknown",
    }
    .to_string();

    match image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
        Ok(reader) => match reader.into_dimensions() {
            Ok((w, h)) => (w, h, format),
            Err(_) => (0, 0, format),
        },
        Err(_) => (0, 0, format),
    }
}

/// Binary/embedded tag keys to skip (album art, lyrics, etc.)
const BINARY_TAG_PATTERNS: &[&str] = &[
    "apic",
    "pic",
    "uslt",
    "sylt",
    "geob",
    "metadata_block_picture",
    "picture",
    "popularimeter",
    "cover",
    "artwork",
    "lyrics",
];

/// Check if a tag key matches binary/embedded patterns that should be filtered.
fn is_binary_tag_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    BINARY_TAG_PATTERNS.iter().any(|&p| lower.contains(p))
}

/// Write tags to a FLAC file via VorbisComments.
fn write_vorbis_tags_flac(path: &Path, tags: &TagSet) -> Result<()> {
    use lofty::ogg::VorbisComments;

    let mut reader = open_buffered(path)?;
    let mut flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default())
        .with_context(|| format!("Failed to read FLAC: {}", path.display()))?;

    // Strip any non-standard ID3v2 tag: lofty can only remove (not write) ID3v2 in FLAC,
    // and save_to_path will error if a non-empty ID3v2 tag is present.
    flac.remove_id3v2();

    let vc = match flac.vorbis_comments_mut() {
        Some(vc) => vc,
        None => {
            flac.set_vorbis_comments(VorbisComments::default());
            flac.vorbis_comments_mut().unwrap()
        }
    };

    populate_vorbis_comments(vc, tags);

    flac.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to FLAC: {}", path.display()))?;

    Ok(())
}

/// Generate a write function for an OGG-based format (Opus, OGG Vorbis).
macro_rules! write_vorbis_ogg_format {
    ($fn_name:ident, $file_type:ty, $format_name:expr) => {
        fn $fn_name(path: &Path, tags: &TagSet) -> Result<()> {
            let mut reader = open_buffered(path)?;
            let mut file = <$file_type>::read_from(&mut reader, ParseOptions::default())
                .with_context(|| format!("Failed to read {}: {}", $format_name, path.display()))?;
            populate_vorbis_comments(file.vorbis_comments_mut(), tags);
            file.save_to_path(path, WriteOptions::default())
                .with_context(|| {
                    format!("Failed to save tags to {}: {}", $format_name, path.display())
                })?;
            Ok(())
        }
    };
}

write_vorbis_ogg_format!(write_vorbis_tags_opus, lofty::ogg::OpusFile, "Opus");
write_vorbis_ogg_format!(write_vorbis_tags_ogg, lofty::ogg::VorbisFile, "OGG");

/// Populate a VorbisComments with all tags from a TagSet.
///
/// Clears existing tags, then pushes each (key, value) pair.
/// Multi-value fields (e.g., multiple genres) are naturally supported
/// since push() allows duplicate keys.
fn populate_vorbis_comments(vc: &mut lofty::ogg::VorbisComments, tags: &TagSet) {
    use lofty::tag::TagExt as _;
    vc.clear();
    for (key, value) in tags.iter() {
        vc.push(key.to_uppercase(), value.to_string());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::tags::DiffClassification;

    // =========================================================================
    // Test fixture helpers
    // =========================================================================

    /// Generate a minimal valid FLAC file with silence at the given path.
    fn generate_flac_fixture(path: &Path) {
        // 4410 samples of silence = 0.1s at 44100 Hz, mono, 16-bit
        let samples = vec![0i32; 4410];

        let mut encoder = flac_codec::encode::FlacSampleWriter::create(
            path,
            flac_codec::encode::Options::default()
                .no_padding()
                .no_seektable(),
            44100,
            16,
            1,
            Some(4410),
        )
        .expect("FLAC encoder");

        encoder.write(&samples).expect("FLAC write");
        encoder.finalize().expect("FLAC finalize");
    }

    /// Generate a minimal valid Opus file with silence at the given path.
    fn generate_opus_fixture(path: &Path) {
        use audiopus::coder::Encoder as OpusEncoder;
        use audiopus::{
            Application, Bitrate, Channels as OpusChannels, SampleRate as OpusSampleRate,
        };
        use ogg::writing::{PacketWriteEndInfo, PacketWriter};
        use std::io::BufWriter;

        const FRAME_SIZE: usize = 960; // 20ms at 48kHz

        let mut encoder = OpusEncoder::new(
            OpusSampleRate::Hz48000,
            OpusChannels::Mono,
            Application::Audio,
        )
        .expect("Opus encoder");
        encoder
            .set_bitrate(Bitrate::BitsPerSecond(64000))
            .expect("set bitrate");

        let pre_skip = encoder.lookahead().expect("lookahead");

        let file = std::fs::File::create(path).expect("create Opus file");
        let mut ogg = PacketWriter::new(BufWriter::new(file));
        let serial: u32 = 42;

        // OpusHead
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(1); // channels (mono)
        head.extend_from_slice(&(pre_skip as u16).to_le_bytes());
        head.extend_from_slice(&48000u32.to_le_bytes());
        head.extend_from_slice(&0u16.to_le_bytes()); // output gain
        head.push(0); // mapping family
        ogg.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
            .expect("write OpusHead");

        // OpusTags
        let vendor = b"mm-test";
        let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        ogg.write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)
            .expect("write OpusTags");

        // Encode 5 frames of silence (100ms)
        let silence = vec![0i16; FRAME_SIZE];
        let mut packet_buf = vec![0u8; 4000];
        let mut granule: u64 = pre_skip as u64;

        for i in 0..5 {
            let len = encoder.encode(&silence, &mut packet_buf).expect("encode");
            granule += FRAME_SIZE as u64;
            let end_info = if i == 4 {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            ogg.write_packet(packet_buf[..len].to_vec(), serial, end_info, granule)
                .expect("write packet");
        }
    }

    /// Path to the checked-in OGG Vorbis silence fixture.
    fn ogg_vorbis_fixture_path() -> std::path::PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest_dir).join("src/corpus/test_fixtures/silence.ogg")
    }

    /// Copy the OGG Vorbis fixture to a temp file for read-write testing.
    fn copy_ogg_vorbis_fixture(dest: &Path) {
        let source = ogg_vorbis_fixture_path();
        assert!(
            source.exists(),
            "OGG Vorbis fixture not found at {:?}",
            source
        );
        std::fs::copy(&source, dest).expect("copy OGG Vorbis fixture");
    }

    /// Standard test tags covering various key types.
    fn standard_test_tags() -> TagSet {
        TagSet::new(vec![
            ("artist".to_string(), "Test Artist".to_string()),
            ("album".to_string(), "Test Album".to_string()),
            ("title".to_string(), "Test Title".to_string()),
            ("genre".to_string(), "Electronic".to_string()),
            ("tracknumber".to_string(), "5".to_string()),
            ("date".to_string(), "2024".to_string()),
            ("comment".to_string(), "Test comment".to_string()),
        ])
    }

    /// Write tags to a file and verify they round-trip without loss.
    fn assert_round_trip(path: &Path, tags: &TagSet, format_name: &str) {
        write_tags_to_file(path, tags).unwrap_or_else(|e| panic!("write tags to {format_name}: {e}"));
        let readback = from_file(path).unwrap_or_else(|e| panic!("read tags from {format_name}: {e}"));
        let diff = tags.diff(&readback);
        assert!(
            diff.only_left.is_empty(),
            "Tags lost in {format_name} round-trip: {:?}",
            diff.only_left.as_slice()
        );
    }

    // =========================================================================
    // TagSet unit tests (no I/O)
    // =========================================================================

    #[test]
    fn test_tagset_new_deduplicates() {
        let tags = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("artist".to_string(), "Foo".to_string()), // duplicate
            ("Artist".to_string(), "Foo".to_string()), // case variation = same after normalize
        ]);
        assert_eq!(tags.len(), 1);
    }

    #[test]
    fn test_tagset_new_sorts() {
        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "Bar".to_string()),
        ]);
        let keys: Vec<&str> = tags.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["ALBUM", "ARTIST", "GENRE"]);
    }

    #[test]
    fn test_tagset_multi_value() {
        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
        ]);
        assert_eq!(tags.len(), 2);
        let genres: Vec<&str> = tags.values_for("genre").collect();
        assert!(genres.contains(&"Rock"));
        assert!(genres.contains(&"Metal"));
    }

    #[test]
    fn test_tagset_contains() {
        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
        ]);
        assert!(tags.contains("genre", "Rock"));
        assert!(tags.contains("GENRE", "Rock")); // case insensitive key
        assert!(!tags.contains("genre", "Jazz"));
    }

    #[test]
    fn test_tagset_skips_empty_values() {
        let tags = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "".to_string()), // empty, should be skipped
        ]);
        assert_eq!(tags.len(), 1);
        assert!(tags.get("album").is_none());
    }

    #[test]
    fn test_tagset_diff_identical() {
        let a = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let b = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let diff = a.diff(&b);
        assert!(diff.only_left.is_empty() && diff.only_right.is_empty());
        assert_eq!(diff.classify(), DiffClassification::Identical);
    }

    #[test]
    fn test_tagset_diff_left_only() {
        let a = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "Bar".to_string()),
        ]);
        let b = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let diff = a.diff(&b);
        assert!(!diff.only_left.is_empty());
        assert_eq!(diff.classify(), DiffClassification::LeftOnly);
        assert_eq!(diff.only_left.len(), 1);
        assert!(diff.only_left.contains("album", "Bar"));
    }

    #[test]
    fn test_tagset_diff_right_only() {
        let a = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let b = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "Bar".to_string()),
        ]);
        let diff = a.diff(&b);
        assert_eq!(diff.classify(), DiffClassification::RightOnly);
        assert!(diff.only_right.contains("album", "Bar"));
    }

    #[test]
    fn test_tagset_diff_conflict() {
        let a = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("extra_a".to_string(), "A".to_string()),
        ]);
        let b = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("extra_b".to_string(), "B".to_string()),
        ]);
        let diff = a.diff(&b);
        assert_eq!(diff.classify(), DiffClassification::Conflict);
        assert!(diff.only_left.contains("extra_a", "A"));
        assert!(diff.only_right.contains("extra_b", "B"));
        // "artist=Foo" is in both, so it should NOT be in only_left or only_right
        assert!(!diff.only_left.contains("artist", "Foo"));
        assert!(!diff.only_right.contains("artist", "Foo"));
    }

    #[test]
    fn test_tagset_diff_value_change() {
        // Same key, different values = conflict (both sides have "extras")
        let a = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let b = TagSet::new(vec![("artist".to_string(), "Bar".to_string())]);
        let diff = a.diff(&b);
        assert_eq!(diff.classify(), DiffClassification::Conflict);
        assert!(diff.only_left.contains("artist", "Foo"));
        assert!(diff.only_right.contains("artist", "Bar"));
    }

    // =========================================================================
    // Binary tag filtering
    // =========================================================================

    #[test]
    fn test_binary_tag_keys_filtered() {
        assert!(is_binary_tag_key("METADATA_BLOCK_PICTURE"));
        assert!(is_binary_tag_key("metadata_block_picture"));
        assert!(is_binary_tag_key("APIC"));
        assert!(is_binary_tag_key("cover"));
        assert!(is_binary_tag_key("LYRICS"));
    }

    #[test]
    fn test_non_binary_tag_keys_not_filtered() {
        assert!(!is_binary_tag_key("ARTIST"));
        assert!(!is_binary_tag_key("SOFTWARE"));
        assert!(!is_binary_tag_key("WEBSITE"));
        assert!(!is_binary_tag_key("REPLAYGAIN_TRACK_GAIN"));
        assert!(!is_binary_tag_key("GENRE"));
        assert!(!is_binary_tag_key("LENGTH"));
    }

    // =========================================================================
    // FLAC round-trip tests
    // =========================================================================

    #[test]
    fn test_flac_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let tags = standard_test_tags();
        assert_round_trip(&path, &tags, "FLAC");
    }

    #[test]
    fn test_flac_nonstandard_keys_round_trip() {
        // These keys caused the 133 ghost OOB signals via ItemKey::Unknown
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let tags = TagSet::new(vec![
            ("software".to_string(), "MM v0.1".to_string()),
            ("website".to_string(), "https://example.com".to_string()),
            ("length".to_string(), "12345".to_string()),
            ("comments".to_string(), "A long comment".to_string()),
            ("replaygain_track_gain".to_string(), "-6.5 dB".to_string()),
            ("replaygain_track_peak".to_string(), "0.987654".to_string()),
            ("encoder".to_string(), "libFLAC 1.3.4".to_string()),
        ]);
        assert_round_trip(&path, &tags, "FLAC nonstandard");
    }

    #[test]
    fn test_flac_multi_value_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
            ("artist".to_string(), "Band A".to_string()),
            ("artist".to_string(), "Band B".to_string()),
        ]);
        write_tags_to_file(&path, &tags).expect("write multi-value tags to FLAC");

        let readback = from_file(&path).expect("read back multi-value tags from FLAC");
        assert_eq!(readback.len(), 4);
        let genres: Vec<&str> = readback.values_for("genre").collect();
        assert!(genres.contains(&"Rock"));
        assert!(genres.contains(&"Metal"));
        let artists: Vec<&str> = readback.values_for("artist").collect();
        assert!(artists.contains(&"Band A"));
        assert!(artists.contains(&"Band B"));
    }

    #[test]
    fn test_flac_no_existing_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        // Fresh FLAC with no tags — write should create tag container
        let tags = TagSet::new(vec![("title".to_string(), "New Song".to_string())]);
        write_tags_to_file(&path, &tags).expect("write to tagless FLAC");

        let readback = from_file(&path).expect("read from FLAC");
        assert!(readback.contains("title", "New Song"));
    }

    #[test]
    fn test_flac_replaces_existing_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        // Write first set
        let tags1 = TagSet::new(vec![
            ("artist".to_string(), "Old Artist".to_string()),
            ("album".to_string(), "Old Album".to_string()),
        ]);
        write_tags_to_file(&path, &tags1).unwrap();

        // Write second set — should completely replace
        let tags2 = TagSet::new(vec![
            ("artist".to_string(), "New Artist".to_string()),
            ("genre".to_string(), "Jazz".to_string()),
        ]);
        write_tags_to_file(&path, &tags2).unwrap();

        let readback = from_file(&path).unwrap();
        assert!(readback.contains("artist", "New Artist"));
        assert!(readback.contains("genre", "Jazz"));
        // Old tags should be gone
        assert!(!readback.contains("artist", "Old Artist"));
        assert!(!readback.contains("album", "Old Album"));
    }

    #[test]
    fn test_flac_corruption_regression() {
        // Write tags, verify file is still a valid FLAC that lofty can parse
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let tags = standard_test_tags();
        write_tags_to_file(&path, &tags).expect("write tags to FLAC");

        // Verify lofty can still read the file as valid FLAC
        let file = std::fs::File::open(&path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        let flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default());
        assert!(
            flac.is_ok(),
            "FLAC file corrupted after tag write: {:?}",
            flac.err()
        );

        // Verify tags survived
        let readback = from_file(&path).unwrap();
        assert!(readback.contains("artist", "Test Artist"));

        // Verify file size is reasonable (not truncated or ballooned)
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 100, "FLAC file suspiciously small: {} bytes", size);
    }

    // =========================================================================
    // Opus round-trip tests
    // =========================================================================

    #[test]
    fn test_opus_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        generate_opus_fixture(&path);

        let tags = standard_test_tags();
        assert_round_trip(&path, &tags, "Opus");
    }

    #[test]
    fn test_opus_nonstandard_keys_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        generate_opus_fixture(&path);

        let tags = TagSet::new(vec![
            ("software".to_string(), "MM v0.1".to_string()),
            ("website".to_string(), "https://example.com".to_string()),
            ("length".to_string(), "12345".to_string()),
            ("replaygain_track_gain".to_string(), "-6.5 dB".to_string()),
        ]);
        assert_round_trip(&path, &tags, "Opus nonstandard");
    }

    #[test]
    fn test_opus_corruption_regression() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        generate_opus_fixture(&path);

        let tags = standard_test_tags();
        write_tags_to_file(&path, &tags).expect("write tags to Opus");

        // Verify symphonia can still probe and identify it
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let file = std::fs::File::open(&path).unwrap();
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("opus");
        let result = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        );
        assert!(
            result.is_ok(),
            "Opus file corrupted after tag write: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_opus_multi_value_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        generate_opus_fixture(&path);

        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
        ]);
        write_tags_to_file(&path, &tags).expect("write multi-value to Opus");

        let readback = from_file(&path).unwrap();
        let genres: Vec<&str> = readback.values_for("genre").collect();
        assert!(genres.contains(&"Rock"));
        assert!(genres.contains(&"Metal"));
    }

    // =========================================================================
    // OGG Vorbis round-trip tests
    // =========================================================================

    #[test]
    fn test_ogg_vorbis_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.ogg");
        copy_ogg_vorbis_fixture(&path);

        let tags = standard_test_tags();
        assert_round_trip(&path, &tags, "OGG Vorbis");
    }

    #[test]
    fn test_ogg_vorbis_nonstandard_keys_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.ogg");
        copy_ogg_vorbis_fixture(&path);

        let tags = TagSet::new(vec![
            ("software".to_string(), "MM v0.1".to_string()),
            ("website".to_string(), "https://example.com".to_string()),
            ("length".to_string(), "12345".to_string()),
            ("replaygain_track_gain".to_string(), "-6.5 dB".to_string()),
        ]);
        assert_round_trip(&path, &tags, "OGG Vorbis nonstandard");
    }

    // =========================================================================
    // Key casing tests
    // =========================================================================

    #[test]
    fn test_key_casing_normalized_on_read() {
        // VorbisComments keys are case-insensitive; TagSet normalizes to UPPERCASE
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        // Write with uppercase keys (populate_vorbis_comments uppercases)
        let tags = TagSet::new(vec![
            ("Artist".to_string(), "Foo".to_string()),
            ("ALBUM".to_string(), "Bar".to_string()),
        ]);
        write_tags_to_file(&path, &tags).unwrap();

        let readback = from_file(&path).unwrap();
        // Keys should be uppercased
        assert!(readback.contains("artist", "Foo"));
        assert!(readback.contains("album", "Bar"));
    }

    // =========================================================================
    // Unicode tests
    // =========================================================================

    #[test]
    fn test_unicode_values_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let tags = TagSet::new(vec![
            (
                "artist".to_string(),
                "\u{4e2d}\u{6587}\u{827a}\u{672f}\u{5bb6}".to_string(),
            ), // CJK
            (
                "album".to_string(),
                "\u{00e9}\u{00e8}\u{00ea}\u{00eb}".to_string(),
            ), // accented chars
            ("title".to_string(), "\u{1f3b5} Music \u{1f3b6}".to_string()), // emoji
        ]);
        assert_round_trip(&path, &tags, "FLAC unicode");
    }

    #[test]
    fn test_unicode_values_opus_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        generate_opus_fixture(&path);

        let tags = TagSet::new(vec![
            (
                "artist".to_string(),
                "\u{4e2d}\u{6587}\u{827a}\u{672f}\u{5bb6}".to_string(),
            ),
            ("title".to_string(), "\u{1f3b5} Music".to_string()),
        ]);
        assert_round_trip(&path, &tags, "Opus unicode");
    }

    // =========================================================================
    // Edge case tests
    // =========================================================================

    #[test]
    fn test_very_long_tag_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let long_value = "x".repeat(100_000); // 100KB value
        let tags = TagSet::new(vec![("comment".to_string(), long_value.clone())]);
        write_tags_to_file(&path, &tags).unwrap();

        let readback = from_file(&path).unwrap();
        assert_eq!(readback.get("comment"), Some(long_value.as_str()));
    }

    #[test]
    fn test_many_unique_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let tags = TagSet::new(
            (0..100)
                .map(|i| (format!("custom_key_{}", i), format!("value_{}", i)))
                .collect::<Vec<_>>(),
        );
        assert_round_trip(&path, &tags, "FLAC 100 keys");
    }

    #[test]
    fn test_write_unsupported_format_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        std::fs::write(&path, b"fake mp3 data").unwrap();

        let tags = standard_test_tags();
        let result = write_tags_to_file(&path, &tags);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cannot write tags to .mp3"), "Got: {}", err);
    }

    #[test]
    fn test_write_wav_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        std::fs::write(&path, b"fake wav data").unwrap();

        let tags = standard_test_tags();
        let result = write_tags_to_file(&path, &tags);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot write tags to .wav"));
    }

    #[test]
    fn test_mtime_changes_after_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        let meta_before = std::fs::metadata(&path).unwrap();
        let mtime_before = meta_before.modified().unwrap();

        // Small sleep to ensure mtime difference
        std::thread::sleep(std::time::Duration::from_millis(50));

        let tags = standard_test_tags();
        write_tags_to_file(&path, &tags).unwrap();

        let meta_after = std::fs::metadata(&path).unwrap();
        let mtime_after = meta_after.modified().unwrap();
        assert!(
            mtime_after > mtime_before,
            "mtime should change after tag write"
        );
    }

    #[test]
    fn test_read_file_with_no_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.flac");
        generate_flac_fixture(&path);

        // Fresh FLAC has no VorbisComments block
        let tags = from_file(&path).unwrap();
        assert!(
            tags.is_empty(),
            "Fresh FLAC should have no tags, got {:?}",
            tags.as_slice()
        );
    }

    #[test]
    fn test_read_opus_with_no_user_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        generate_opus_fixture(&path);

        // Fresh Opus has OpusTags header but no user tags
        let tags = from_file(&path).unwrap();
        assert!(
            tags.is_empty(),
            "Fresh Opus should have no user tags, got {:?}",
            tags.as_slice()
        );
    }

    #[test]
    fn test_from_vorbis_comments_filters_binary_keys() {
        // Construct VorbisComments with a binary-pattern key
        let mut vc = lofty::ogg::VorbisComments::default();
        vc.push("ARTIST".to_string(), "Foo".to_string());
        vc.push(
            "METADATA_BLOCK_PICTURE".to_string(),
            "base64data".to_string(),
        );
        vc.push("TITLE".to_string(), "Bar".to_string());

        let tags = tagset_from_vorbis_comments(Some(&vc));
        assert_eq!(tags.len(), 2);
        assert!(tags.contains("artist", "Foo"));
        assert!(tags.contains("title", "Bar"));
        assert!(!tags.contains("metadata_block_picture", "base64data"));
    }

    // =========================================================================
    // values_for_normalized tests
    // =========================================================================

    #[test]
    fn test_values_for_normalized_single_variant() {
        let tags = TagSet::new(vec![
            ("ALBUMARTIST".to_string(), "Test Artist".to_string()),
            ("TITLE".to_string(), "Song".to_string()),
        ]);
        let results = tags.values_for_normalized("album_artist");
        assert_eq!(results, vec![("ALBUMARTIST", "Test Artist")]);
    }

    #[test]
    fn test_values_for_normalized_multi_variant() {
        // File has both ALBUMARTIST and ALBUM_ARTIST (the bug scenario)
        let tags = TagSet::new(vec![
            ("ALBUMARTIST".to_string(), "Artist A".to_string()),
            ("ALBUM_ARTIST".to_string(), "Artist B".to_string()),
        ]);
        let results = tags.values_for_normalized("ALBUMARTIST");
        assert_eq!(results.len(), 2);
        // Both variants should be found
        assert!(results.contains(&("ALBUMARTIST", "Artist A")));
        assert!(results.contains(&("ALBUM_ARTIST", "Artist B")));
    }

    #[test]
    fn test_values_for_normalized_no_match() {
        let tags = TagSet::new(vec![
            ("ARTIST".to_string(), "Test".to_string()),
            ("ALBUM".to_string(), "Album".to_string()),
        ]);
        let results = tags.values_for_normalized("album_artist");
        assert!(results.is_empty());
    }
}
