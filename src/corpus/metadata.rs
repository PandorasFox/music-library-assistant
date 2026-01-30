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
            crate::logging::log_error(&error_msg);
            None
        }
    };

    // Note: Tag fields (artist, album, title, etc.) are stored separately in track_tags table.
    // Use corpus::tags::TagSet::from_file() to get tags from the file if needed.
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

// =============================================================================
// Tag Operations - MOVED TO corpus/tags.rs
// =============================================================================
// All tag reading, writing, and comparison now goes through corpus::tags module.
// See corpus/tags.rs for TagSet, write_file_tags(), and apply_edits_to_file().

