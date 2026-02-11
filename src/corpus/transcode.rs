//! Native audio transcoding.
//!
//! Decodes audio via symphonia and re-encodes to FLAC (via flacenc, pure Rust) or
//! Opus (via audiopus + ogg + rubato). No external subprocess required.

use anyhow::{Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::Signal;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Target format for transcoding operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscodeTarget {
    /// Opus in OGG container at specified bitrate (VBR).
    Opus { bitrate_kbps: u32 },
    /// FLAC (lossless).
    Flac,
    /// Losslessly capture a lossy source's waveform into FLAC.
    FlacLossyCapture,
}

impl TranscodeTarget {
    /// File extension for the target format (container type).
    pub fn extension(&self) -> &'static str {
        match self {
            TranscodeTarget::Opus { .. } => "opus",
            TranscodeTarget::Flac | TranscodeTarget::FlacLossyCapture => "flac",
        }
    }

    /// Compute the destination path for a transcode of the given source.
    ///
    /// - `Opus`/`Flac`: replace extension (e.g. `song.wav` -> `song.flac`)
    /// - `FlacLossyCapture`: append `.LOSSY.flac` (e.g. `song.mp3` -> `song.mp3.LOSSY.flac`)
    pub fn dest_path(&self, source: &Path) -> PathBuf {
        match self {
            TranscodeTarget::Opus { .. } | TranscodeTarget::Flac => {
                source.with_extension(self.extension())
            }
            TranscodeTarget::FlacLossyCapture => {
                let orig_ext = source
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                let new_ext = format!("{}.LOSSY.flac", orig_ext);
                source.with_extension(new_ext)
            }
        }
    }
}

/// Transcode a source audio file to the target format using native decode/encode.
///
/// The destination path must not already exist.
pub fn transcode(source: &Path, dest: &Path, target: TranscodeTarget) -> Result<()> {
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    if dest.exists() {
        return Err(anyhow::anyhow!(
            "Destination file already exists: {}",
            dest.display()
        ));
    }

    // Create parent directory if needed
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Decode source with symphonia and encode to target
    match target {
        TranscodeTarget::Flac | TranscodeTarget::FlacLossyCapture => {
            encode_flac(source, dest)?;
        }
        TranscodeTarget::Opus { bitrate_kbps } => {
            encode_opus(source, dest, bitrate_kbps)?;
        }
    }

    // Copy tags from source to destination via lofty
    copy_tags(source, dest)
        .with_context(|| format!(
            "Transcode succeeded but tag copy failed: {} -> {}",
            source.display(),
            dest.display()
        ))?;

    // Verify the output file was created
    if !dest.exists() {
        return Err(anyhow::anyhow!(
            "Transcode completed but output file was not created: {}",
            dest.display()
        ));
    }

    Ok(())
}

// ============================================================================
// FLAC encoding (via flacenc — pure Rust, batch encoder)
// ============================================================================

fn encode_flac(source: &Path, dest: &Path) -> Result<()> {
    use flacenc::bitsink::ByteSink;
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    use flacenc::source::MemSource;

    let (mut format, mut decoder, sample_rate, channels, bits_per_sample) =
        open_source(source)?;

    // flacenc is batch-only: collect all decoded samples first
    let mut all_samples: Vec<i32> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let (samples, _frames) = convert_audio_buffer_to_i32(decoded, channels)?;
                all_samples.extend_from_slice(&samples);
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow::anyhow!("Decode error: {}", e)),
        }
    }

    if all_samples.is_empty() {
        return Err(anyhow::anyhow!("No audio frames decoded from source"));
    }

    let source = MemSource::from_samples(
        &all_samples,
        channels,
        bits_per_sample as usize,
        sample_rate as usize,
    );

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_enc, e)| anyhow::anyhow!("FLAC config verification failed: {:?}", e))?;

    let stream = flacenc::encode_with_fixed_block_size(&config, source, 4096)
        .map_err(|e| anyhow::anyhow!("FLAC encoding failed: {:?}", e))?;

    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| anyhow::anyhow!("FLAC stream write failed: {:?}", e))?;

    std::fs::write(dest, sink.as_slice())
        .with_context(|| format!("Failed to write FLAC output: {}", dest.display()))?;

    Ok(())
}

// ============================================================================
// Opus encoding (via audiopus + ogg + rubato)
// ============================================================================

fn encode_opus(source: &Path, dest: &Path, bitrate_kbps: u32) -> Result<()> {
    use audiopus::coder::Encoder as OpusEncoder;
    use audiopus::{Application, Bitrate, Channels as OpusChannels, SampleRate as OpusSampleRate};
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};
    use std::io::BufWriter;

    const FRAME_SIZE: usize = 960; // 20ms at 48kHz

    let (mut format, mut decoder, sample_rate, channels, _bps) = open_source(source)?;

    if channels > 2 {
        return Err(anyhow::anyhow!(
            "Opus encoding supports mono/stereo only, got {} channels",
            channels
        ));
    }

    // Collect all decoded samples
    let mut all_samples: Vec<i16> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let samples = convert_audio_buffer_to_i16(decoded, channels)?;
                all_samples.extend_from_slice(&samples);
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow::anyhow!("Decode error: {}", e)),
        }
    }

    if all_samples.is_empty() {
        return Err(anyhow::anyhow!("No audio frames decoded from source"));
    }

    // Resample to 48kHz if needed (Opus requires specific sample rates)
    let samples_48k = if sample_rate == 48000 {
        all_samples
    } else {
        resample_to_48k(&all_samples, sample_rate, channels)?
    };

    // Set up Opus encoder
    let opus_channels = if channels == 1 {
        OpusChannels::Mono
    } else {
        OpusChannels::Stereo
    };

    let mut encoder =
        OpusEncoder::new(OpusSampleRate::Hz48000, opus_channels, Application::Audio)
            .map_err(|e| anyhow::anyhow!("Failed to create Opus encoder: {:?}", e))?;

    encoder
        .set_bitrate(Bitrate::BitsPerSecond(bitrate_kbps as i32 * 1000))
        .map_err(|e| anyhow::anyhow!("Failed to set Opus bitrate: {:?}", e))?;

    let pre_skip = encoder
        .lookahead()
        .map_err(|e| anyhow::anyhow!("Failed to get Opus lookahead: {:?}", e))?;

    // Set up OGG muxer
    let file = File::create(dest)
        .with_context(|| format!("Failed to create output file: {}", dest.display()))?;
    let mut ogg = PacketWriter::new(BufWriter::new(file));
    let serial: u32 = rand::random();

    // OpusHead (ID header)
    let opus_head = build_opus_head(channels as u8, pre_skip as u16, sample_rate);
    ogg.write_packet(opus_head, serial, PacketWriteEndInfo::EndPage, 0)
        .context("Failed to write OpusHead")?;

    // OpusTags (comment header — lofty overwrites this in copy_tags)
    let opus_tags = build_opus_tags();
    ogg.write_packet(opus_tags, serial, PacketWriteEndInfo::EndPage, 0)
        .context("Failed to write OpusTags")?;

    // Encode audio in 960-sample frames
    let samples_per_frame = FRAME_SIZE * channels;
    let total_interleaved = samples_48k.len();
    let total_frames_count =
        (total_interleaved + samples_per_frame - 1) / samples_per_frame;

    let mut packet_buf = vec![0u8; 4000];
    let mut granule_pos: u64 = pre_skip as u64;

    for i in 0..total_frames_count {
        let start = i * samples_per_frame;
        let end = std::cmp::min(start + samples_per_frame, total_interleaved);

        // Pad last frame with silence if needed
        let frame: std::borrow::Cow<[i16]> = if end - start < samples_per_frame {
            let mut padded = vec![0i16; samples_per_frame];
            padded[..end - start].copy_from_slice(&samples_48k[start..end]);
            std::borrow::Cow::Owned(padded)
        } else {
            std::borrow::Cow::Borrowed(&samples_48k[start..end])
        };

        let encoded_len = encoder
            .encode(&frame, &mut packet_buf)
            .map_err(|e| anyhow::anyhow!("Opus encode failed: {:?}", e))?;

        granule_pos += FRAME_SIZE as u64;

        let is_last = i == total_frames_count - 1;
        let end_info = if is_last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };

        ogg.write_packet(packet_buf[..encoded_len].to_vec(), serial, end_info, granule_pos)
            .context("Failed to write Opus audio packet")?;
    }

    Ok(())
}

/// Build the 19-byte OpusHead identification header (RFC 7845 §5.1).
fn build_opus_head(channels: u8, pre_skip: u16, input_sample_rate: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(channels);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&input_sample_rate.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes()); // output gain
    head.push(0); // mapping family 0 (mono/stereo)
    head
}

/// Build a minimal OpusTags comment header (RFC 7845 §5.2).
fn build_opus_tags() -> Vec<u8> {
    let vendor = b"mm-native";
    let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // 0 user comments
    tags
}

/// Resample interleaved i16 audio from `source_rate` to 48000 Hz.
fn resample_to_48k(samples: &[i16], source_rate: u32, channels: usize) -> Result<Vec<i16>> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
        WindowFunction,
    };

    let ratio = 48000.0 / source_rate as f64;
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    };

    let chunk_size = 1024;
    let mut resampler = SincFixedIn::<f64>::new(ratio, 2.0, params, chunk_size, channels)
        .map_err(|e| anyhow::anyhow!("Failed to create resampler: {:?}", e))?;

    // Deinterleave i16 → per-channel f64
    let total_frames = samples.len() / channels;
    let mut channel_data: Vec<Vec<f64>> = vec![Vec::with_capacity(total_frames); channels];
    for frame in 0..total_frames {
        for ch in 0..channels {
            channel_data[ch].push(samples[frame * channels + ch] as f64 / 32768.0);
        }
    }

    // Process full chunks
    let mut output_channels: Vec<Vec<f64>> = vec![Vec::new(); channels];
    let mut pos = 0;
    while pos + chunk_size <= total_frames {
        let chunk: Vec<&[f64]> = channel_data.iter().map(|ch| &ch[pos..pos + chunk_size]).collect();
        let resampled = resampler
            .process(&chunk, None)
            .map_err(|e| anyhow::anyhow!("Resample failed: {:?}", e))?;
        for (ch_idx, ch_data) in resampled.into_iter().enumerate() {
            output_channels[ch_idx].extend(ch_data);
        }
        pos += chunk_size;
    }

    // Process remaining samples (partial last chunk)
    if pos < total_frames {
        let chunk: Vec<&[f64]> = channel_data.iter().map(|ch| &ch[pos..]).collect();
        let resampled = resampler
            .process_partial(Some(&chunk), None)
            .map_err(|e| anyhow::anyhow!("Resample tail failed: {:?}", e))?;
        for (ch_idx, ch_data) in resampled.into_iter().enumerate() {
            output_channels[ch_idx].extend(ch_data);
        }
    }

    // Reinterleave f64 → i16
    let output_frames = output_channels[0].len();
    let mut result = Vec::with_capacity(output_frames * channels);
    for frame in 0..output_frames {
        for ch in 0..channels {
            let sample = (output_channels[ch][frame] * 32768.0).clamp(-32768.0, 32767.0) as i16;
            result.push(sample);
        }
    }

    Ok(result)
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Open a source file with symphonia, returning the format reader, decoder,
/// and audio parameters (sample_rate, channels, bits_per_sample).
fn open_source(
    source: &Path,
) -> Result<(
    Box<dyn symphonia::core::formats::FormatReader>,
    Box<dyn symphonia::core::codecs::Decoder>,
    u32,
    usize,
    u32,
)> {
    let file = File::open(source)
        .with_context(|| format!("Failed to open source: {}", source.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = source.extension() {
        if let Some(ext_str) = ext.to_str() {
            hint.with_extension(ext_str);
        }
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .with_context(|| format!("Failed to probe audio: {}", source.display()))?;

    let format = probed.format;
    let track = format
        .default_track()
        .context("No default audio track found")?;

    let sample_rate = track
        .codec_params
        .sample_rate
        .context("No sample rate in source")?;
    let channels = track
        .codec_params
        .channels
        .context("No channel info in source")?
        .count();
    // Default to 16 bits if not specified (common for lossy decoders)
    let bits_per_sample = track.codec_params.bits_per_sample.unwrap_or(16);

    let decoder =
        crate::corpus::codecs::make_decoder(&track.codec_params, &Default::default())
            .context("Failed to create decoder")?;

    Ok((format, decoder, sample_rate, channels, bits_per_sample))
}

/// Convert a symphonia AudioBufferRef to interleaved i32 samples for FLAC encoding.
///
/// Returns (interleaved_samples, frame_count).
fn convert_audio_buffer_to_i32(
    decoded: AudioBufferRef,
    channels: usize,
) -> Result<(Vec<i32>, usize)> {
    match decoded {
        AudioBufferRef::S16(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame] as i32);
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::S32(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame]);
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::F32(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    // Scale f32 [-1.0, 1.0] to i32 range for 16-bit depth
                    let sample = (buf.chan(ch)[frame] * 32767.0).clamp(-32768.0, 32767.0) as i32;
                    samples.push(sample);
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::F64(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    let sample = (buf.chan(ch)[frame] * 32767.0).clamp(-32768.0, 32767.0) as i32;
                    samples.push(sample);
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::U8(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push((buf.chan(ch)[frame] as i32 - 128) * 256);
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::U16(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame] as i32 - 32768);
                }
            }
            Ok((samples, frames))
        }
        _ => Err(anyhow::anyhow!("Unsupported audio buffer format")),
    }
}

/// Convert a symphonia AudioBufferRef to interleaved i16 samples for Opus encoding.
fn convert_audio_buffer_to_i16(decoded: AudioBufferRef, channels: usize) -> Result<Vec<i16>> {
    match decoded {
        AudioBufferRef::S16(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame]);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::F32(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    let sample = (buf.chan(ch)[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::S32(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push((buf.chan(ch)[frame] >> 16) as i16);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::F64(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    let sample = (buf.chan(ch)[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    samples.push(sample);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::U8(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(((buf.chan(ch)[frame] as i32 - 128) * 256) as i16);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::U16(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push((buf.chan(ch)[frame] as i32 - 32768) as i16);
                }
            }
            Ok(samples)
        }
        _ => Err(anyhow::anyhow!("Unsupported audio buffer format")),
    }
}

/// Copy tags from source file to destination file via lofty.
///
/// Reads all text tags from the source and writes them to the destination.
/// This replaces ffmpeg's implicit tag mapping for the native transcode pipeline.
fn copy_tags(source: &Path, dest: &Path) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemValue, Tag, TagItem, TagType};

    // Read tags from source
    let source_file = Probe::open(source)
        .with_context(|| format!("Failed to open source for tag reading: {}", source.display()))?
        .read()
        .with_context(|| format!("Failed to read source tags: {}", source.display()))?;

    let source_tag = match source_file.primary_tag() {
        Some(t) => t,
        None => return Ok(()), // No tags to copy
    };

    // Open destination and get/create its primary tag
    let mut dest_file = Probe::open(dest)
        .with_context(|| format!("Failed to open dest for tag writing: {}", dest.display()))?
        .read()
        .with_context(|| format!("Failed to read dest tags: {}", dest.display()))?;

    let dest_tag_type = dest_file.primary_tag_type();
    if dest_file.primary_tag().is_none() {
        dest_file.insert_tag(Tag::new(dest_tag_type));
    }
    let dest_tag = dest_file.primary_tag_mut().unwrap();

    // Copy standard accessor fields
    if let Some(v) = source_tag.artist() {
        dest_tag.set_artist(v.to_string());
    }
    if let Some(v) = source_tag.title() {
        dest_tag.set_title(v.to_string());
    }
    if let Some(v) = source_tag.album() {
        dest_tag.set_album(v.to_string());
    }
    if let Some(v) = source_tag.genre() {
        dest_tag.set_genre(v.to_string());
    }
    if let Some(v) = source_tag.track() {
        dest_tag.set_track(v);
    }
    if let Some(v) = source_tag.year() {
        dest_tag.set_year(v);
    }

    // Copy all items (extended tags, multi-value)
    // Skip binary items (album art, etc.) — we only want text metadata
    for item in source_tag.items() {
        match item.value() {
            ItemValue::Text(s) => {
                let new_item = TagItem::new(item.key().clone(), ItemValue::Text(s.clone()));
                dest_tag.push(new_item);
            }
            ItemValue::Locator(s) => {
                let new_item = TagItem::new(item.key().clone(), ItemValue::Locator(s.clone()));
                dest_tag.push(new_item);
            }
            ItemValue::Binary(_) => continue,
        }
    }

    // Remove legacy ID3v1 if present
    dest_file.remove(TagType::Id3v1);

    dest_file
        .save_to_path(dest, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to: {}", dest.display()))?;

    Ok(())
}
