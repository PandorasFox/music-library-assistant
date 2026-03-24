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
///
/// For MP3, reads ID3v2 frames directly and maps them to Vorbis Comment names
/// using the Picard tag mapping convention.
///
/// For other formats (m4a, etc.), falls back to lofty's generic Tag/Probe.
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
        "mp3" => {
            let mut reader = open_buffered(path)?;
            let mp3 = lofty::mpeg::MpegFile::read_from(&mut reader, ParseOptions::default())
                .with_context(|| format!("Failed to read tags from: {}", path.display()))?;
            Ok(tagset_from_id3v2(mp3.id3v2()))
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
        "mp3" => {
            use lofty::id3::v2::Frame;
            use lofty::picture::PictureInformation;

            let file = std::fs::File::open(path).ok()?;
            let mut reader = std::io::BufReader::new(file);
            let mp3 =
                lofty::mpeg::MpegFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            let id3v2 = mp3.id3v2()?;

            let apic_pics: Vec<&lofty::picture::Picture> = id3v2
                .into_iter()
                .filter_map(|f| match f {
                    Frame::Picture(apic) => Some(&*apic.picture),
                    _ => None,
                })
                .collect();

            if apic_pics.is_empty() {
                return None;
            }

            let count = apic_pics.len() as u32;
            let pic: &&lofty::picture::Picture = apic_pics
                .iter()
                .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
                .or_else(|| apic_pics.first())
                .unwrap();

            let info = PictureInformation::from_picture(pic).unwrap_or_default();
            Some(PictureInfo {
                format: mime_type_to_format(pic.mime_type()),
                width: info.width,
                height: info.height,
                count,
            })
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
        "mp3" => write_id3v2_tags_mp3(path, tags),
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
/// Uses the `imagesize` crate — reads only the file header to determine
/// dimensions without decoding any pixel data.
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

    match imagesize::size(path) {
        Ok(dims) => (dims.width as u32, dims.height as u32, format),
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
// ID3v2 (MP3) — Mapping Tables and Read/Write
// =============================================================================
//
// Bidirectional mapping between Vorbis Comment names (our canonical internal
// representation) and ID3v2 frames, following the MusicBrainz Picard convention:
// https://picard-docs.musicbrainz.org/en/latest/appendices/tag_mapping.html

/// UFID owner string for MusicBrainz recording IDs.
/// (Not re-exported from lofty — it's pub(super) there.)
const MUSICBRAINZ_UFID_OWNER: &str = "http://musicbrainz.org";

/// Vorbis Comment name -> ID3v2 standard text frame ID.
const VORBIS_TO_ID3V2_TEXT: &[(&str, &str)] = &[
    ("ALBUM", "TALB"),
    ("ALBUMARTIST", "TPE2"),
    ("ALBUMARTISTSORT", "TSO2"),
    ("ALBUMSORT", "TSOA"),
    ("ARTIST", "TPE1"),
    ("ARTISTSORT", "TSOP"),
    ("BPM", "TBPM"),
    ("COMPILATION", "TCMP"),
    ("COMPOSER", "TCOM"),
    ("COMPOSERSORT", "TSOC"),
    ("CONDUCTOR", "TPE3"),
    ("COPYRIGHT", "TCOP"),
    ("DISCSUBTITLE", "TSST"),
    ("ENCODEDBY", "TENC"),
    ("ENCODERSETTINGS", "TSSE"),
    ("GENRE", "TCON"),
    ("ISRC", "TSRC"),
    ("KEY", "TKEY"),
    ("LABEL", "TPUB"),
    ("LANGUAGE", "TLAN"),
    ("LYRICIST", "TEXT"),
    ("MEDIA", "TMED"),
    ("MOOD", "TMOO"),
    ("MOVEMENT", "MVIN"),
    ("MOVEMENTNAME", "MVNM"),
    ("REMIXER", "TPE4"),
    ("SUBTITLE", "TIT3"),
    ("TITLE", "TIT2"),
    ("TITLESORT", "TSOT"),
];

/// Vorbis Comment name -> TXXX frame description.
const VORBIS_TO_TXXX: &[(&str, &str)] = &[
    ("ACOUSTID_FINGERPRINT", "Acoustid Fingerprint"),
    ("ACOUSTID_ID", "Acoustid Id"),
    ("ASIN", "ASIN"),
    ("BARCODE", "BARCODE"),
    ("CATALOGNUMBER", "CATALOGNUMBER"),
    ("MUSICBRAINZ_ALBUMARTISTID", "MusicBrainz Album Artist Id"),
    ("MUSICBRAINZ_ALBUMID", "MusicBrainz Album Id"),
    ("MUSICBRAINZ_ARTISTID", "MusicBrainz Artist Id"),
    ("MUSICBRAINZ_DISCID", "MusicBrainz Disc Id"),
    ("MUSICBRAINZ_RELEASEGROUPID", "MusicBrainz Release Group Id"),
    ("MUSICBRAINZ_RELEASETRACKID", "MusicBrainz Release Track Id"),
    ("MUSICBRAINZ_WORKID", "MusicBrainz Work Id"),
    ("RELEASECOUNTRY", "MusicBrainz Album Release Country"),
    ("RELEASESTATUS", "MusicBrainz Album Status"),
    ("RELEASETYPE", "MusicBrainz Album Type"),
    ("REPLAYGAIN_ALBUM_GAIN", "REPLAYGAIN_ALBUM_GAIN"),
    ("REPLAYGAIN_ALBUM_PEAK", "REPLAYGAIN_ALBUM_PEAK"),
    ("REPLAYGAIN_ALBUM_RANGE", "REPLAYGAIN_ALBUM_RANGE"),
    ("REPLAYGAIN_REFERENCE_LOUDNESS", "REPLAYGAIN_REFERENCE_LOUDNESS"),
    ("REPLAYGAIN_TRACK_GAIN", "REPLAYGAIN_TRACK_GAIN"),
    ("REPLAYGAIN_TRACK_PEAK", "REPLAYGAIN_TRACK_PEAK"),
    ("REPLAYGAIN_TRACK_RANGE", "REPLAYGAIN_TRACK_RANGE"),
    ("SCRIPT", "SCRIPT"),
    ("WORK", "WORK"),
];

/// Timestamp frame ID -> Vorbis Comment name.
const ID3V2_TIMESTAMP_TO_VORBIS: &[(&str, &str)] = &[
    ("TDRC", "DATE"),
    ("TDOR", "ORIGINALDATE"),
];

/// Look up the Vorbis Comment name for an ID3v2 text frame ID.
fn id3v2_text_to_vorbis(frame_id: &str) -> Option<&'static str> {
    VORBIS_TO_ID3V2_TEXT
        .iter()
        .find(|(_, id3)| *id3 == frame_id)
        .map(|(vorbis, _)| *vorbis)
}

/// Look up the Vorbis Comment name for a TXXX description (case-insensitive).
fn txxx_description_to_vorbis(description: &str) -> Option<&'static str> {
    VORBIS_TO_TXXX
        .iter()
        .find(|(_, desc)| desc.eq_ignore_ascii_case(description))
        .map(|(vorbis, _)| *vorbis)
}

/// Look up the ID3v2 text frame ID for a Vorbis Comment name.
fn vorbis_to_id3v2_text(vorbis_key: &str) -> Option<&'static str> {
    VORBIS_TO_ID3V2_TEXT
        .iter()
        .find(|(vorbis, _)| *vorbis == vorbis_key)
        .map(|(_, id3)| *id3)
}

/// Look up the TXXX description for a Vorbis Comment name.
fn vorbis_to_txxx_description(vorbis_key: &str) -> Option<&'static str> {
    VORBIS_TO_TXXX
        .iter()
        .find(|(vorbis, _)| *vorbis == vorbis_key)
        .map(|(_, desc)| *desc)
}

/// Build a TagSet from an ID3v2 tag by mapping frames to Vorbis Comment names.
fn tagset_from_id3v2(tag: Option<&lofty::id3::v2::Id3v2Tag>) -> TagSet {
    use lofty::id3::v2::Frame;

    let Some(tag) = tag else {
        return TagSet::empty();
    };
    let mut pairs: Vec<(String, String)> = Vec::new();

    for frame in tag {
        match frame {
            Frame::Text(text_frame) => {
                let frame_id = text_frame.id().as_str();

                // TRCK and TPOS encode number/total as "N/T"
                if frame_id == "TRCK" || frame_id == "TPOS" {
                    id3v2_split_number_pair(frame_id, &text_frame.value, &mut pairs);
                    continue;
                }

                if let Some(vorbis_key) = id3v2_text_to_vorbis(frame_id) {
                    // Split null-separated multi-values (ID3v2.4 convention)
                    for value in text_frame.value.split('\0') {
                        let v = value.trim();
                        if !v.is_empty() {
                            pairs.push((vorbis_key.to_string(), v.to_string()));
                        }
                    }
                }
                // Unknown text frames are skipped — no lossless Vorbis mapping
            }
            Frame::UserText(txxx) => {
                let vorbis_key = txxx_description_to_vorbis(&txxx.description)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| txxx.description.to_uppercase());

                if is_binary_tag_key(&vorbis_key) {
                    continue;
                }
                let v = txxx.content.trim();
                if !v.is_empty() {
                    pairs.push((vorbis_key, v.to_string()));
                }
            }
            Frame::Timestamp(ts_frame) => {
                let frame_id = ts_frame.id().as_str();
                if let Some((_, vorbis_key)) = ID3V2_TIMESTAMP_TO_VORBIS
                    .iter()
                    .find(|(id3, _)| *id3 == frame_id)
                {
                    pairs.push((vorbis_key.to_string(), ts_frame.timestamp.to_string()));
                }
            }
            Frame::UniqueFileIdentifier(ufid) => {
                if ufid.owner.as_ref() == MUSICBRAINZ_UFID_OWNER {
                    if let Ok(uuid_str) = std::str::from_utf8(&ufid.identifier) {
                        let v = uuid_str.trim();
                        if !v.is_empty() {
                            pairs.push(("MUSICBRAINZ_TRACKID".to_string(), v.to_string()));
                        }
                    }
                }
            }
            _ => {} // Skip APIC, COMM, USLT, POPM, binary, etc.
        }
    }

    TagSet::new(pairs)
}

/// Split an ID3v2 TRCK or TPOS value ("N/T" or "N") into separate tag pairs.
fn id3v2_split_number_pair(
    frame_id: &str,
    value: &str,
    pairs: &mut Vec<(String, String)>,
) {
    let (number_key, total_key) = match frame_id {
        "TRCK" => ("TRACKNUMBER", "TOTALTRACKS"),
        "TPOS" => ("DISCNUMBER", "TOTALDISCS"),
        _ => return,
    };

    if let Some((num, total)) = value.split_once('/') {
        let num = num.trim();
        if !num.is_empty() && num != "0" {
            pairs.push((number_key.to_string(), num.to_string()));
        }
        let total = total.trim();
        if !total.is_empty() && total != "0" {
            pairs.push((total_key.to_string(), total.to_string()));
        }
    } else {
        let num = value.trim();
        if !num.is_empty() {
            pairs.push((number_key.to_string(), num.to_string()));
        }
    }
}

/// Write tags to an MP3 file via ID3v2 frames.
///
/// Maps Vorbis Comment names to ID3v2 frame types using the Picard convention.
/// Preserves existing APIC (picture) frames. Removes stale ID3v1 and APE tags.
fn write_id3v2_tags_mp3(path: &Path, tags: &TagSet) -> Result<()> {
    use lofty::id3::v2::{Frame, Id3v2Tag};

    let mut reader = open_buffered(path)?;
    let mut mp3 = lofty::mpeg::MpegFile::read_from(&mut reader, ParseOptions::default())
        .with_context(|| format!("Failed to read MP3: {}", path.display()))?;

    // Preserve existing picture frames
    let existing_pictures: Vec<Frame<'static>> = mp3
        .id3v2()
        .map(|tag| {
            tag.into_iter()
                .filter(|f| matches!(f, Frame::Picture(_)))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let mut id3v2 = Id3v2Tag::new();

    // Re-insert preserved pictures
    for pic_frame in existing_pictures {
        id3v2.insert(pic_frame);
    }

    populate_id3v2_from_tagset(&mut id3v2, tags);

    mp3.set_id3v2(id3v2);
    mp3.remove_id3v1();
    mp3.remove_ape();

    mp3.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to MP3: {}", path.display()))?;

    Ok(())
}

/// Populate an Id3v2Tag from a TagSet.
///
/// Groups contiguous identical keys (TagSet is sorted) and maps each to the
/// appropriate ID3v2 frame type:
/// - Standard text frames (TIT2, TPE1, etc.) with null-separated multi-values
/// - TXXX frames for MusicBrainz IDs, ReplayGain, and unknown keys
/// - UFID frame for MusicBrainz recording ID
/// - Timestamp frames for DATE and ORIGINALDATE
/// - TRCK/TPOS for track/disc number+total pairs
fn populate_id3v2_from_tagset(id3v2: &mut lofty::id3::v2::Id3v2Tag, tags: &TagSet) {
    use lofty::id3::v2::{Frame, FrameId, TextInformationFrame, TimestampFrame,
                          UniqueFileIdentifierFrame};
    use std::borrow::Cow;

    // Collect number pair components across the full TagSet
    let mut tracknumber: Option<&str> = None;
    let mut totaltracks: Option<&str> = None;
    let mut discnumber: Option<&str> = None;
    let mut totaldiscs: Option<&str> = None;

    // Group by key — TagSet is sorted, so identical keys are contiguous
    let mut grouped: Vec<(&str, Vec<&str>)> = Vec::new();
    for (key, value) in tags.iter() {
        if let Some(last) = grouped.last_mut() {
            if last.0 == key {
                last.1.push(value);
                continue;
            }
        }
        grouped.push((key, vec![value]));
    }

    for (key, values) in &grouped {
        match *key {
            "TRACKNUMBER" => tracknumber = Some(values[0]),
            "TOTALTRACKS" | "TRACKTOTAL" => totaltracks = Some(values[0]),
            "DISCNUMBER" => discnumber = Some(values[0]),
            "TOTALDISCS" | "DISCTOTAL" => totaldiscs = Some(values[0]),
            "MUSICBRAINZ_TRACKID" => {
                id3v2.insert(Frame::UniqueFileIdentifier(
                    UniqueFileIdentifierFrame::new(
                        MUSICBRAINZ_UFID_OWNER,
                        values[0].as_bytes().to_vec(),
                    ),
                ));
            }
            "DATE" => {
                if let Ok(ts) = values[0].parse::<lofty::tag::items::Timestamp>() {
                    id3v2.insert(Frame::Timestamp(TimestampFrame::new(
                        FrameId::Valid(Cow::Borrowed("TDRC")),
                        lofty::TextEncoding::UTF8,
                        ts,
                    )));
                }
            }
            "ORIGINALDATE" => {
                if let Ok(ts) = values[0].parse::<lofty::tag::items::Timestamp>() {
                    id3v2.insert(Frame::Timestamp(TimestampFrame::new(
                        FrameId::Valid(Cow::Borrowed("TDOR")),
                        lofty::TextEncoding::UTF8,
                        ts,
                    )));
                }
            }
            other => {
                if let Some(frame_id) = vorbis_to_id3v2_text(other) {
                    let joined = values.join("\0");
                    id3v2.insert(Frame::Text(TextInformationFrame::new(
                        FrameId::Valid(Cow::Borrowed(frame_id)),
                        lofty::TextEncoding::UTF8,
                        joined,
                    )));
                } else {
                    let description = vorbis_to_txxx_description(other)
                        .unwrap_or(other);
                    let joined = values.join("\0");
                    id3v2.insert_user_text(description.to_string(), joined);
                }
            }
        }
    }

    // Emit TRCK (track number / total)
    if tracknumber.is_some() || totaltracks.is_some() {
        let pair_str = format_number_pair(tracknumber, totaltracks);
        id3v2.insert(Frame::Text(TextInformationFrame::new(
            FrameId::Valid(Cow::Borrowed("TRCK")),
            lofty::TextEncoding::UTF8,
            pair_str,
        )));
    }

    // Emit TPOS (disc number / total)
    if discnumber.is_some() || totaldiscs.is_some() {
        let pair_str = format_number_pair(discnumber, totaldiscs);
        id3v2.insert(Frame::Text(TextInformationFrame::new(
            FrameId::Valid(Cow::Borrowed("TPOS")),
            lofty::TextEncoding::UTF8,
            pair_str,
        )));
    }
}

/// Format a number/total pair for TRCK or TPOS frames.
fn format_number_pair(number: Option<&str>, total: Option<&str>) -> String {
    match (number, total) {
        (Some(n), Some(t)) => format!("{}/{}", n, t),
        (Some(n), None) => n.to_string(),
        (None, Some(t)) => format!("0/{}", t),
        (None, None) => String::new(),
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
        let path = dir.path().join("test.m4a");
        std::fs::write(&path, b"fake m4a data").unwrap();

        let tags = standard_test_tags();
        let result = write_tags_to_file(&path, &tags);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cannot write tags to .m4a"), "Got: {}", err);
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

    // =========================================================================
    // MP3 (ID3v2) fixture and tests
    // =========================================================================

    /// Generate a minimal valid MP3 file (MPEG1 Layer3 silence frames).
    fn generate_mp3_fixture(path: &Path) {
        // MPEG1 Layer3 frame header: 128kbps, 44100Hz, mono, no CRC, no padding
        // Byte 0: 0xFF (sync)
        // Byte 1: 0xFB = 11111011 (sync + MPEG1 + Layer3 + no CRC)
        // Byte 2: 0x90 = 10010000 (128kbps + 44100Hz + no padding)
        // Byte 3: 0xC0 = 11000000 (mono + no extension)
        let header: [u8; 4] = [0xFF, 0xFB, 0x90, 0xC0];
        // Frame size = floor(144 * 128000 / 44100) = 417 bytes
        let frame_size = 417;
        let mut frame = vec![0u8; frame_size];
        frame[..4].copy_from_slice(&header);

        let mut data = Vec::with_capacity(frame_size * 10);
        for _ in 0..10 {
            data.extend_from_slice(&frame);
        }
        std::fs::write(path, &data).expect("write MP3 fixture");
    }

    #[test]
    fn test_mp3_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = standard_test_tags();
        assert_round_trip(&path, &tags, "MP3");
    }

    #[test]
    fn test_mp3_musicbrainz_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("MUSICBRAINZ_TRACKID".to_string(), "12345678-1234-1234-1234-123456789abc".to_string()),
            ("MUSICBRAINZ_ALBUMID".to_string(), "abcdef01-2345-6789-abcd-ef0123456789".to_string()),
            ("MUSICBRAINZ_ARTISTID".to_string(), "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()),
            ("MUSICBRAINZ_RELEASETRACKID".to_string(), "11111111-2222-3333-4444-555555555555".to_string()),
            ("ACOUSTID_ID".to_string(), "deadbeef-cafe-babe-feed-facecafebabe".to_string()),
        ]);
        assert_round_trip(&path, &tags, "MP3 MusicBrainz");
    }

    #[test]
    fn test_mp3_number_pair_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("TRACKNUMBER".to_string(), "5".to_string()),
            ("TOTALTRACKS".to_string(), "12".to_string()),
            ("DISCNUMBER".to_string(), "1".to_string()),
            ("TOTALDISCS".to_string(), "2".to_string()),
        ]);
        write_tags_to_file(&path, &tags).expect("write number pair tags");

        let readback = from_file(&path).expect("read back number pair tags");
        assert_eq!(readback.get("TRACKNUMBER"), Some("5"), "tracknumber");
        assert_eq!(readback.get("TOTALTRACKS"), Some("12"), "totaltracks");
        assert_eq!(readback.get("DISCNUMBER"), Some("1"), "discnumber");
        assert_eq!(readback.get("TOTALDISCS"), Some("2"), "totaldiscs");
    }

    #[test]
    fn test_mp3_number_pair_number_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("TRACKNUMBER".to_string(), "3".to_string()),
        ]);
        write_tags_to_file(&path, &tags).expect("write track number only");

        let readback = from_file(&path).expect("read back track number");
        assert_eq!(readback.get("TRACKNUMBER"), Some("3"));
        assert!(readback.get("TOTALTRACKS").is_none());
    }

    #[test]
    fn test_mp3_multi_value_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("GENRE".to_string(), "Electronic".to_string()),
            ("GENRE".to_string(), "Ambient".to_string()),
            ("ARTIST".to_string(), "Artist A".to_string()),
            ("ARTIST".to_string(), "Artist B".to_string()),
        ]);
        write_tags_to_file(&path, &tags).expect("write multi-value tags to MP3");

        let readback = from_file(&path).expect("read back multi-value tags from MP3");
        let genres: Vec<&str> = readback.values_for("GENRE").collect();
        assert!(genres.contains(&"Electronic"), "genres: {:?}", genres);
        assert!(genres.contains(&"Ambient"), "genres: {:?}", genres);
        let artists: Vec<&str> = readback.values_for("ARTIST").collect();
        assert!(artists.contains(&"Artist A"), "artists: {:?}", artists);
        assert!(artists.contains(&"Artist B"), "artists: {:?}", artists);
    }

    #[test]
    fn test_mp3_timestamp_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("DATE".to_string(), "2024".to_string()),
            ("ORIGINALDATE".to_string(), "2020".to_string()),
        ]);
        write_tags_to_file(&path, &tags).expect("write timestamp tags");

        let readback = from_file(&path).expect("read back timestamp tags");
        assert_eq!(readback.get("DATE"), Some("2024"));
        assert_eq!(readback.get("ORIGINALDATE"), Some("2020"));
    }

    #[test]
    fn test_mp3_unknown_tags_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        // Tags without a known mapping go to TXXX with key as description
        let tags = TagSet::new(vec![
            ("CUSTOM_TAG".to_string(), "custom value".to_string()),
            ("MYAPP_VERSION".to_string(), "1.0".to_string()),
        ]);
        assert_round_trip(&path, &tags, "MP3 unknown tags");
    }

    #[test]
    fn test_mp3_replaces_existing_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        // Write first set
        let tags1 = TagSet::new(vec![
            ("ARTIST".to_string(), "Old Artist".to_string()),
            ("ALBUM".to_string(), "Old Album".to_string()),
        ]);
        write_tags_to_file(&path, &tags1).unwrap();

        // Write second set — should completely replace
        let tags2 = TagSet::new(vec![
            ("ARTIST".to_string(), "New Artist".to_string()),
            ("GENRE".to_string(), "Jazz".to_string()),
        ]);
        write_tags_to_file(&path, &tags2).unwrap();

        let readback = from_file(&path).unwrap();
        assert!(readback.contains("ARTIST", "New Artist"));
        assert!(readback.contains("GENRE", "Jazz"));
        assert!(!readback.contains("ARTIST", "Old Artist"));
        assert!(!readback.contains("ALBUM", "Old Album"));
    }

    #[test]
    fn test_mp3_corruption_regression() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = standard_test_tags();
        write_tags_to_file(&path, &tags).expect("write tags to MP3");

        // Verify lofty can still read the file as valid MPEG
        let file = std::fs::File::open(&path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        let mp3 = lofty::mpeg::MpegFile::read_from(&mut reader, ParseOptions::default());
        assert!(
            mp3.is_ok(),
            "MP3 file corrupted after tag write: {:?}",
            mp3.err()
        );

        let readback = from_file(&path).unwrap();
        assert!(readback.contains("ARTIST", "Test Artist"));
    }

    #[test]
    fn test_mp3_no_existing_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        // Fresh MP3 with no ID3v2 tag
        let tags = from_file(&path).unwrap();
        assert!(
            tags.is_empty(),
            "Fresh MP3 should have no tags, got {:?}",
            tags.as_slice()
        );
    }

    #[test]
    fn test_mp3_replaygain_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("REPLAYGAIN_TRACK_GAIN".to_string(), "-6.5 dB".to_string()),
            ("REPLAYGAIN_TRACK_PEAK".to_string(), "0.987654".to_string()),
            ("REPLAYGAIN_ALBUM_GAIN".to_string(), "-8.2 dB".to_string()),
        ]);
        assert_round_trip(&path, &tags, "MP3 ReplayGain");
    }

    #[test]
    fn test_mp3_unicode_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mp3");
        generate_mp3_fixture(&path);

        let tags = TagSet::new(vec![
            ("ARTIST".to_string(), "\u{4e2d}\u{6587}\u{827a}\u{672f}\u{5bb6}".to_string()),
            ("ALBUM".to_string(), "\u{00e9}\u{00e8}\u{00ea}\u{00eb}".to_string()),
            ("TITLE".to_string(), "\u{1f3b5} Music \u{1f3b6}".to_string()),
        ]);
        assert_round_trip(&path, &tags, "MP3 unicode");
    }

    // =========================================================================
    // ID3v2 mapping unit tests (no file I/O)
    // =========================================================================

    #[test]
    fn test_id3v2_text_mapping_bidirectional() {
        // Forward: Vorbis -> ID3v2
        assert_eq!(vorbis_to_id3v2_text("TITLE"), Some("TIT2"));
        assert_eq!(vorbis_to_id3v2_text("ARTIST"), Some("TPE1"));
        assert_eq!(vorbis_to_id3v2_text("ALBUM"), Some("TALB"));
        assert_eq!(vorbis_to_id3v2_text("ALBUMARTIST"), Some("TPE2"));
        assert_eq!(vorbis_to_id3v2_text("GENRE"), Some("TCON"));
        assert_eq!(vorbis_to_id3v2_text("NONEXISTENT"), None);

        // Reverse: ID3v2 -> Vorbis
        assert_eq!(id3v2_text_to_vorbis("TIT2"), Some("TITLE"));
        assert_eq!(id3v2_text_to_vorbis("TPE1"), Some("ARTIST"));
        assert_eq!(id3v2_text_to_vorbis("TALB"), Some("ALBUM"));
        assert_eq!(id3v2_text_to_vorbis("ZZZZ"), None);
    }

    #[test]
    fn test_txxx_mapping_bidirectional() {
        // Forward: Vorbis -> TXXX description
        assert_eq!(
            vorbis_to_txxx_description("MUSICBRAINZ_ALBUMID"),
            Some("MusicBrainz Album Id")
        );
        assert_eq!(
            vorbis_to_txxx_description("ACOUSTID_ID"),
            Some("Acoustid Id")
        );
        assert_eq!(vorbis_to_txxx_description("NONEXISTENT"), None);

        // Reverse: TXXX description -> Vorbis (case-insensitive)
        assert_eq!(
            txxx_description_to_vorbis("MusicBrainz Album Id"),
            Some("MUSICBRAINZ_ALBUMID")
        );
        assert_eq!(
            txxx_description_to_vorbis("musicbrainz album id"),
            Some("MUSICBRAINZ_ALBUMID")
        );
        assert_eq!(
            txxx_description_to_vorbis("Acoustid Id"),
            Some("ACOUSTID_ID")
        );
    }

    #[test]
    fn test_format_number_pair() {
        assert_eq!(format_number_pair(Some("5"), Some("12")), "5/12");
        assert_eq!(format_number_pair(Some("3"), None), "3");
        assert_eq!(format_number_pair(None, Some("10")), "0/10");
        assert_eq!(format_number_pair(None, None), "");
    }

    #[test]
    fn test_id3v2_split_number_pair() {
        let mut pairs = Vec::new();

        id3v2_split_number_pair("TRCK", "5/12", &mut pairs);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&("TRACKNUMBER".to_string(), "5".to_string())));
        assert!(pairs.contains(&("TOTALTRACKS".to_string(), "12".to_string())));

        pairs.clear();
        id3v2_split_number_pair("TRCK", "7", &mut pairs);
        assert_eq!(pairs.len(), 1);
        assert!(pairs.contains(&("TRACKNUMBER".to_string(), "7".to_string())));

        pairs.clear();
        id3v2_split_number_pair("TPOS", "1/2", &mut pairs);
        assert!(pairs.contains(&("DISCNUMBER".to_string(), "1".to_string())));
        assert!(pairs.contains(&("TOTALDISCS".to_string(), "2".to_string())));
    }
}
