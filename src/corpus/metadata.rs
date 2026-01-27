use anyhow::{Context, Result};
use std::fs;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::config::is_audio_extension;
use crate::corpus::db::Track;

pub struct AudioMetadata {
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub title: Option<String>,
    pub track_number: Option<i32>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
}

pub fn extract_metadata(path: &Path, source: &str) -> Result<Track> {
    // Get file system metadata
    let fs_metadata = fs::metadata(path)
        .with_context(|| format!("Failed to read file metadata: {}", path.display()))?;

    let inode = fs_metadata.ino() as i64;
    let file_size = fs_metadata.len() as i64;

    // Determine file type from extension
    let file_type = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_lowercase();

    // Skip non-audio files
    if !is_audio_file(&file_type) {
        anyhow::bail!("Not an audio file: {}", path.display());
    }

    // Extract audio metadata using Symphonia
    let audio_meta = extract_audio_metadata(path)?;

    // Generate chromaprint fingerprint (non-fatal if it fails)
    let fingerprint = match generate_fingerprint(path) {
        Ok(fp) => Some(fp),
        Err(e) => {
            // Log warning but continue - fingerprinting is optional
            let error_msg = format!(
                "Failed to generate fingerprint for {}: {}",
                path.display(),
                e
            );
            let _ = crate::config::log_scan_error(&error_msg);
            None
        }
    };

    // Note: Tag fields (artist, album, title, etc.) are stored separately in track_tags table.
    // Use read_all_tags() to get tags from the file if needed.
    Ok(Track {
        id: None,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        inode,
        file_size,
        file_type,
        duration_ms: audio_meta.duration_ms,
        bitrate_kbps: audio_meta.bitrate_kbps,
        sample_rate: audio_meta.sample_rate,
        fingerprint,
    })
}

fn is_audio_file(extension: &str) -> bool {
    is_audio_extension(extension)
}

fn extract_audio_metadata(path: &Path) -> Result<AudioMetadata> {
    let file = fs::File::open(path)
        .with_context(|| format!("Failed to open audio file: {}", path.display()))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .context("Failed to probe audio format")?;

    let mut format = probed.format;

    let track = format
        .default_track()
        .context("No default audio track found")?;

    // Extract technical metadata
    let codec_params = &track.codec_params;
    let sample_rate = codec_params.sample_rate;

    // Calculate duration
    let duration_ms = if let Some(n_frames) = codec_params.n_frames {
        if let Some(sr) = sample_rate {
            let duration_secs = n_frames as f64 / sr as f64;
            Some((duration_secs * 1000.0) as i64)
        } else {
            None
        }
    } else {
        None
    };

    // Calculate bitrate from file size and duration if available
    let bitrate_kbps = if let (Some(dur_ms), Ok(file_meta)) = (duration_ms, std::fs::metadata(path))
    {
        if dur_ms > 0 {
            let file_size_bits = file_meta.len() as f64 * 8.0;
            let duration_secs = dur_ms as f64 / 1000.0;
            Some((file_size_bits / duration_secs / 1000.0) as i32)
        } else {
            None
        }
    } else {
        None
    };

    // Extract tags from metadata
    let mut artist = None;
    let mut album = None;
    let mut album_artist = None;
    let mut title = None;
    let mut track_number = None;
    let mut genre = None;
    let mut isrc = None;

    if let Some(metadata_rev) = format.metadata().current() {
        for tag in metadata_rev.tags() {
            match tag.std_key {
                Some(symphonia::core::meta::StandardTagKey::Artist) => {
                    artist = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::Album) => {
                    album = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::AlbumArtist) => {
                    album_artist = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::TrackTitle) => {
                    title = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::TrackNumber) => {
                    if let Ok(num) = tag.value.to_string().parse::<i32>() {
                        track_number = Some(num);
                    }
                }
                Some(symphonia::core::meta::StandardTagKey::Genre) => {
                    genre = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::IdentIsrc) => {
                    isrc = Some(tag.value.to_string());
                }
                _ => {}
            }
        }
    }

    Ok(AudioMetadata {
        duration_ms,
        bitrate_kbps,
        sample_rate: sample_rate.map(|sr| sr as i32),
        artist,
        album,
        album_artist,
        title,
        track_number,
        genre,
        isrc,
    })
}

/// Generate chromaprint fingerprint for audio file.
/// Returns the raw u32 fingerprint values (stored as BLOB in DB).
fn generate_fingerprint(path: &Path) -> Result<Vec<u32>> {
    use rusty_chromaprint::{Configuration, Fingerprinter};

    // Open audio file with symphonia
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        if let Some(ext_str) = ext.to_str() {
            hint.with_extension(ext_str);
        }
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .context("Failed to probe audio format for fingerprinting")?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .context("No default audio track found")?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .context("Failed to create decoder")?;

    // Get audio parameters
    let sample_rate = track
        .codec_params
        .sample_rate
        .context("No sample rate found")?;
    let channels = track
        .codec_params
        .channels
        .context("No channels found")?
        .count();

    // Initialize chromaprint fingerprinter
    let config = Configuration::preset_test2();
    let mut fingerprinter = Fingerprinter::new(&config);

    fingerprinter
        .start(sample_rate, channels as u32)
        .context("Failed to start fingerprinter")?;

    // Decode and feed audio to fingerprinter
    // Limit to first 120 seconds for performance
    const MAX_DURATION_SECS: u64 = 120;
    let max_packets = (MAX_DURATION_SECS * sample_rate as u64) / 1024; // rough estimate
    let mut packet_count = 0;

    while let Ok(packet) = format.next_packet() {
        if packet_count >= max_packets {
            break;
        }
        packet_count += 1;

        match decoder.decode(&packet) {
            Ok(decoded) => {
                // Convert audio buffer to i16 samples
                let samples = match convert_audio_buffer_to_i16(decoded) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Feed samples to chromaprint
                fingerprinter.consume(&samples);
            }
            Err(_) => continue,
        }
    }

    // Finish processing
    fingerprinter.finish();

    // Return fingerprint as Vec<u32> (will be stored as BLOB in DB)
    Ok(fingerprinter.fingerprint().to_vec())
}

/// Convert Symphonia AudioBuffer to i16 samples for chromaprint
fn convert_audio_buffer_to_i16(decoded: AudioBufferRef) -> Result<Vec<i16>> {
    use symphonia::core::audio::Signal;

    match decoded {
        AudioBufferRef::S16(buf) => {
            // Already i16, interleave channels
            let num_frames = buf.frames();
            let num_channels = buf.spec().channels.count();
            let mut samples = Vec::with_capacity(num_frames * num_channels);

            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    samples.push(buf.chan(ch)[frame]);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::F32(buf) => {
            // Convert f32 to i16
            let num_frames = buf.frames();
            let num_channels = buf.spec().channels.count();
            let mut samples = Vec::with_capacity(num_frames * num_channels);

            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    let sample = (buf.chan(ch)[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::U8(buf) => {
            // Convert u8 to i16
            let num_frames = buf.frames();
            let num_channels = buf.spec().channels.count();
            let mut samples = Vec::with_capacity(num_frames * num_channels);

            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    // u8 range 0-255 -> i16 range -32768 to 32767
                    let sample = ((buf.chan(ch)[frame] as i32 - 128) * 256) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::U16(buf) => {
            // Convert u16 to i16
            let num_frames = buf.frames();
            let num_channels = buf.spec().channels.count();
            let mut samples = Vec::with_capacity(num_frames * num_channels);

            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    // u16 range 0-65535 -> i16 range -32768 to 32767
                    let sample = (buf.chan(ch)[frame] as i32 - 32768) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::S32(buf) => {
            // Convert s32 to i16
            let num_frames = buf.frames();
            let num_channels = buf.spec().channels.count();
            let mut samples = Vec::with_capacity(num_frames * num_channels);

            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    // Down-sample from 32-bit to 16-bit
                    let sample = (buf.chan(ch)[frame] >> 16) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::F64(buf) => {
            // Convert f64 to i16
            let num_frames = buf.frames();
            let num_channels = buf.spec().channels.count();
            let mut samples = Vec::with_capacity(num_frames * num_channels);

            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    let sample = (buf.chan(ch)[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        _ => Err(anyhow::anyhow!(
            "Unsupported audio buffer format for fingerprinting"
        )),
    }
}

/// Convert a lofty ItemKey to a clean tag name string.
/// Handles Unknown("NAME") variants properly instead of using Debug format.
fn item_key_to_string(key: &lofty::tag::ItemKey) -> String {
    use lofty::tag::ItemKey;
    match key {
        ItemKey::Unknown(s) => s.clone(),
        ItemKey::TrackArtist => "artist".to_string(),
        ItemKey::AlbumArtist => "album_artist".to_string(),
        ItemKey::TrackTitle => "title".to_string(),
        ItemKey::AlbumTitle => "album".to_string(),
        ItemKey::TrackNumber => "track_number".to_string(),
        ItemKey::DiscNumber => "disc_number".to_string(),
        ItemKey::Genre => "genre".to_string(),
        ItemKey::Year => "year".to_string(),
        ItemKey::RecordingDate => "date".to_string(),
        ItemKey::Comment => "comment".to_string(),
        ItemKey::Composer => "composer".to_string(),
        ItemKey::Conductor => "conductor".to_string(),
        ItemKey::Label => "label".to_string(),
        ItemKey::Remixer => "remixer".to_string(),
        ItemKey::Lyricist => "lyricist".to_string(),
        ItemKey::Writer => "writer".to_string(),
        ItemKey::Bpm => "bpm".to_string(),
        ItemKey::CatalogNumber => "catalog_number".to_string(),
        ItemKey::Barcode => "barcode".to_string(),
        ItemKey::Isrc => "isrc".to_string(),
        ItemKey::MusicBrainzTrackId => "musicbrainz_trackid".to_string(),
        ItemKey::MusicBrainzRecordingId => "musicbrainz_recordingid".to_string(),
        ItemKey::MusicBrainzReleaseId => "musicbrainz_releaseid".to_string(),
        ItemKey::MusicBrainzArtistId => "musicbrainz_artistid".to_string(),
        ItemKey::MusicBrainzReleaseArtistId => "musicbrainz_releaseartistid".to_string(),
        ItemKey::MusicBrainzReleaseGroupId => "musicbrainz_releasegroupid".to_string(),
        ItemKey::MusicBrainzWorkId => "musicbrainz_workid".to_string(),
        // For any other variants, use Debug format but strip the enum name
        other => {
            let debug = format!("{:?}", other);
            // If debug looks like "SomeVariant", just lowercase it
            // If it looks like "Unknown(\"..\")", extract the content
            if debug.starts_with("Unknown(") {
                debug
                    .strip_prefix("Unknown(\"")
                    .and_then(|s| s.strip_suffix("\")"))
                    .map(|s| s.to_string())
                    .unwrap_or(debug)
            } else {
                debug.to_lowercase()
            }
        }
    }
}

/// Read all tags from an audio file using lofty.
/// Returns a vector of (tag_name, tag_value) tuples, deduplicated.
/// Tag names are normalized (lowercase, clean format - not Debug format).
pub fn read_all_tags(path: &Path) -> Result<Vec<(String, String)>> {
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::Accessor;
    use std::collections::HashSet;

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag reading: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    // Binary/embedded tags to skip (album art, lyrics, etc.)
    const BINARY_TAG_PATTERNS: &[&str] = &[
        "apic", "pic", "uslt", "sylt", "geob",
        "metadata_block_picture", "picture", "popularimeter",
        "cover", "artwork", "lyrics",
    ];

    let mut all_tags = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Helper to add tag if not already seen
    let mut add_tag = |name: String, value: String| {
        let normalized = name.to_lowercase();
        // Skip binary patterns
        if BINARY_TAG_PATTERNS.iter().any(|&p| normalized.contains(p)) {
            return;
        }
        // Skip if empty value
        if value.is_empty() {
            return;
        }
        // Only add if not seen (dedup)
        if !seen.contains(&normalized) {
            seen.insert(normalized);
            all_tags.push((name, value));
        }
    };

    // Get the primary tag (first available)
    if let Some(tag) = tagged_file.primary_tag() {
        // Standard tags via Accessor trait (these take priority)
        if let Some(artist) = tag.artist() {
            add_tag("artist".to_string(), artist.as_ref().to_string());
        }
        if let Some(album) = tag.album() {
            add_tag("album".to_string(), album.as_ref().to_string());
        }
        if let Some(title) = tag.title() {
            add_tag("title".to_string(), title.as_ref().to_string());
        }
        if let Some(track) = tag.track() {
            add_tag("track_number".to_string(), track.to_string());
        }
        if let Some(year) = tag.year() {
            add_tag("year".to_string(), year.to_string());
        }
        if let Some(genre) = tag.genre() {
            add_tag("genre".to_string(), genre.as_ref().to_string());
        }

        // Additional fields from items (extended tags)
        for item in tag.items() {
            let key = item_key_to_string(item.key());

            // Extract actual string value from ItemValue
            let value = match item.value() {
                lofty::tag::ItemValue::Text(s) => s.clone(),
                lofty::tag::ItemValue::Locator(s) => s.clone(),
                lofty::tag::ItemValue::Binary(_) => continue, // Skip binary data
            };

            add_tag(key, value);
        }
    }

    // Sort alphabetically by tag name
    all_tags.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    Ok(all_tags)
}

/// Write tags to an audio file using lofty.
///
/// This function ONLY writes to disk. It does NOT update the database.
/// Database updates are the responsibility of the mutation executor that calls this.
///
/// # Authorization
///
/// Requires a `MutationToken` to prove the caller is executing within a mutation context.
/// This prevents external code from bypassing the mutation system.
pub fn write_tags_to_file(
    path: &Path,
    tags: &[(String, String)],
    _token: &crate::corpus::mutations::MutationToken,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
    use std::collections::HashMap;

    // 1. PRESERVE: Read existing tags from file BEFORE making changes
    let existing_tags = read_all_tags(path)
        .with_context(|| format!("Failed to read existing tags from {}", path.display()))?;

    // 2. MERGE: Build map of updates and preserve non-updated tags
    let updates: HashMap<&str, &str> = tags.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let mut final_tags: Vec<(String, String)> = Vec::new();

    // Add all updates from the user
    for (key, value) in tags {
        final_tags.push((key.clone(), value.clone()));
    }

    // Add existing tags that aren't being updated (PRESERVE)
    for (key, value) in existing_tags {
        if !updates.contains_key(key.as_str()) {
            final_tags.push((key, value));
        }
    }

    // 3. Write to file with ALL tags (updated + preserved)
    let mut tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag writing: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    // Get the tag type before we take a mutable borrow (to avoid borrowing conflicts)
    let tag_type = tagged_file.primary_tag_type();

    // Get or create primary tag
    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            // Create a new tag if none exists
            let new_tag = Tag::new(tag_type);
            tagged_file.insert_tag(new_tag);
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    // Clear existing tags before writing merged set
    tag.clear();

    // Write ALL tags (both updated and preserved)
    for (key, value) in &final_tags {
        match key.as_str() {
            "artist" => tag.set_artist(value.to_string()),
            "album" => tag.set_album(value.to_string()),
            "title" => tag.set_title(value.to_string()),
            "track_number" => {
                if let Ok(num) = value.parse::<u32>() {
                    tag.set_track(num);
                }
            }
            "year" | "date" => {
                if let Ok(year) = value.parse::<u32>() {
                    tag.set_year(year);
                }
            }
            "genre" => tag.set_genre(value.to_string()),
            // SUPPORT ARBITRARY TAGS: Use insert_text for non-standard tags
            _ => {
                // Try to map to a standard ItemKey for the file's tag type
                let item_key = ItemKey::from_key(tag_type, key);
                tag.insert_text(item_key, value.to_string());
            }
        }
    }

    // Save to file with default write options
    tagged_file
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to file: {}", path.display()))?;

    Ok(())
}

/// Write tags to an audio file, supporting multi-value tags.
///
/// Unlike `write_tags_to_file`, this function:
/// - Accepts duplicate keys in the input (e.g., multiple genre values)
/// - Writes them as separate tag entries (for Vorbis/FLAC)
/// - Preserves existing tags that aren't being replaced
///
/// For tags being replaced, ALL existing values for that tag name are removed
/// and replaced with the new values.
///
/// # Multi-value support
///
/// Vorbis comments (FLAC, OGG) natively support multiple values per tag.
/// ID3v2 has limited support. Other formats may only keep the last value.
///
/// # Authorization
///
/// Requires a `MutationToken` to prove the caller is executing within a mutation context.
pub fn write_tags_to_file_multi_value(
    path: &Path,
    tags: &[(String, String)],
    _token: &crate::corpus::mutations::MutationToken,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemKey, ItemValue, Tag, TagExt, TagItem};
    use std::collections::HashSet;

    // 1. Read existing tags from file
    let existing_tags = read_all_tags(path)
        .with_context(|| format!("Failed to read existing tags from {}", path.display()))?;

    // 2. Determine which tag names are being replaced
    let replaced_keys: HashSet<String> = tags.iter().map(|(k, _)| k.to_lowercase()).collect();

    // 3. Open file for writing
    let mut tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag writing: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    let tag_type = tagged_file.primary_tag_type();

    // Get or create primary tag
    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            let new_tag = Tag::new(tag_type);
            tagged_file.insert_tag(new_tag);
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    // Clear existing tags before writing
    tag.clear();

    // 4. Write preserved tags (existing tags not being replaced)
    for (key, value) in &existing_tags {
        if replaced_keys.contains(&key.to_lowercase()) {
            continue; // Skip - this tag is being replaced
        }
        write_single_tag(tag, tag_type, key, value);
    }

    // 5. Write new tags (including multi-value)
    for (key, value) in tags {
        if value.is_empty() {
            continue; // Skip empty values
        }
        write_single_tag(tag, tag_type, key, value);
    }

    // 6. Save to file
    tagged_file
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to file: {}", path.display()))?;

    Ok(())
}

/// Write a single tag value to a tag container.
///
/// For multi-value support, this uses push_item() which adds without replacing,
/// allowing multiple values for the same key (e.g., multiple genres).
fn write_single_tag(
    tag: &mut lofty::tag::Tag,
    tag_type: lofty::tag::TagType,
    key: &str,
    value: &str,
) {
    use lofty::tag::{Accessor, ItemKey, ItemValue, TagItem};

    // For standard tags, we still use set_* for the first value
    // but for multi-value we need to use push_item
    let item_key = match key.to_lowercase().as_str() {
        "artist" => ItemKey::TrackArtist,
        "album" => ItemKey::AlbumTitle,
        "album_artist" => ItemKey::AlbumArtist,
        "title" => ItemKey::TrackTitle,
        "track_number" => ItemKey::TrackNumber,
        "disc_number" => ItemKey::DiscNumber,
        "year" | "date" => ItemKey::Year,
        "genre" => ItemKey::Genre,
        "comment" => ItemKey::Comment,
        "composer" => ItemKey::Composer,
        _ => ItemKey::from_key(tag_type, key),
    };

    // Create tag item and push (allows duplicates for multi-value)
    let item = TagItem::new(item_key, ItemValue::Text(value.to_string()));
    tag.push(item);
}

