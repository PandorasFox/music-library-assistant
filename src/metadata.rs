#![allow(dead_code)]

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

use crate::db::Track;

pub struct AudioMetadata {
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub title: Option<String>,
    pub track_number: Option<i32>,
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

    Ok(Track {
        id: None,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        inode,
        file_size,
        file_type,
        artist: audio_meta.artist,
        album: audio_meta.album,
        album_artist: audio_meta.album_artist,
        title: audio_meta.title,
        track_number: audio_meta.track_number,
        duration_ms: audio_meta.duration_ms,
        bitrate_kbps: audio_meta.bitrate_kbps,
        sample_rate: audio_meta.sample_rate,
        fingerprint,
        isrc: audio_meta.isrc,
    })
}

fn is_audio_file(extension: &str) -> bool {
    matches!(
        extension,
        "mp3" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wav" | "wma" | "ape" | "wv"
    )
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
        isrc,
    })
}

/// Generate chromaprint fingerprint for audio file
fn generate_fingerprint(path: &Path) -> Result<String> {
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

    // Get fingerprint as u32 array
    let fp_data = fingerprinter.fingerprint();

    // Convert u32 array to a string representation (comma-separated)
    let fingerprint = fp_data
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Ok(fingerprint)
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

/// Read all tags from an audio file using lofty
/// Returns a vector of (tag_name, tag_value) tuples
pub fn read_all_tags(path: &Path) -> Result<Vec<(String, String)>> {
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::Accessor;

    let tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag reading: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    // Binary/embedded tags to skip (album art, lyrics, etc.)
    const BINARY_TAG_TYPES: &[&str] = &[
        "APIC",                   // Album art (ID3v2)
        "PIC",                    // Picture (ID3v1)
        "USLT",                   // Unsynchronized lyrics
        "SYLT",                   // Synchronized lyrics
        "GEOB",                   // General encapsulated object
        "METADATA_BLOCK_PICTURE", // FLAC/Vorbis album art
        "Picture",                // Generic picture tag
        "Popularimeter",          // Rating/playcount data (often large)
    ];

    let mut all_tags = Vec::new();

    // Get the primary tag (first available)
    if let Some(tag) = tagged_file.primary_tag() {
        // Standard tags via Accessor trait
        if let Some(artist) = tag.artist() {
            all_tags.push(("artist".to_string(), artist.as_ref().to_string()));
        }
        if let Some(album) = tag.album() {
            all_tags.push(("album".to_string(), album.as_ref().to_string()));
        }
        if let Some(title) = tag.title() {
            all_tags.push(("title".to_string(), title.as_ref().to_string()));
        }
        if let Some(track) = tag.track() {
            all_tags.push(("track_number".to_string(), track.to_string()));
        }
        if let Some(year) = tag.year() {
            all_tags.push(("year".to_string(), year.to_string()));
        }
        if let Some(genre) = tag.genre() {
            all_tags.push(("genre".to_string(), genre.as_ref().to_string()));
        }

        // Additional fields from items (allow duplicates)
        for item in tag.items() {
            let key = format!("{:?}", item.key());

            // Skip binary/embedded tags (album art, lyrics, etc.)
            if BINARY_TAG_TYPES
                .iter()
                .any(|&binary_type| key.contains(binary_type))
            {
                continue;
            }

            let value = format!("{:?}", item.value());

            // Always add, allow duplicates (needed for multiple album_artist tags)
            all_tags.push((key, value));
        }
    }

    // Also check other tags if present (allow duplicates)
    for tag in tagged_file.tags() {
        for item in tag.items() {
            let key = format!("{:?}", item.key());

            // Skip binary/embedded tags (album art, lyrics, etc.)
            if BINARY_TAG_TYPES
                .iter()
                .any(|&binary_type| key.contains(binary_type))
            {
                continue;
            }

            let value = format!("{:?}", item.value());

            // Always add, allow duplicates (needed for multiple album_artist tags)
            all_tags.push((key, value));
        }
    }

    Ok(all_tags)
}

/// Write tags to an audio file using lofty
/// Updates: file tags -> tag_edit_history -> database
pub fn write_tags(
    path: &Path,
    tags: &[(String, String)],
    track_id: i64,
    session_id: &str,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
    use std::collections::HashMap;

    // 1. PRESERVE: Read existing tags from file BEFORE making changes
    let existing_tags = read_all_tags(path).unwrap_or_default();

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

    // 2. Write to tag_edit_history table
    if let Ok(db_path) = crate::config::get_db_path() {
        if let Ok(db) = crate::db::Database::open(&db_path) {
            // Write history entries for each tag change
            for (field_name, new_value) in tags {
                let _ = db.log_tag_edit(track_id, field_name, None, Some(new_value), session_id);
            }
        }
    }

    // 3. Update database (last, as it can be rebuilt with scan)
    if let Ok(db_path) = crate::config::get_db_path() {
        if let Ok(db) = crate::db::Database::open(&db_path) {
            // Update the tracks table with new tag values
            for (field_name, new_value) in tags {
                let _ = db.update_track_tag(track_id, field_name, new_value);
            }
        }
    }

    Ok(())
}
