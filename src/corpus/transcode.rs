//! Native audio transcoding.
//!
//! Decodes audio via symphonia and re-encodes to FLAC (via flac-codec, pure Rust) or
//! Opus (via audiopus + ogg + rubato). No external subprocess required.

use anyhow::{Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::witch::MutationExecutionWitness;

use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::Signal;

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
pub fn transcode(source: &Path, dest: &Path, target: TranscodeTarget, _witness: &MutationExecutionWitness) -> Result<()> {
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

    // Copy pictures (album art) from source into destination before text tags.
    // For FLAC destinations, these become PICTURE metadata blocks that survive
    // the subsequent text tag write (lofty reads them into FlacFile.pictures on
    // parse and chains them back into output on save).
    copy_pictures(source, dest)
        .with_context(|| format!(
            "Transcode succeeded but picture copy failed: {} -> {}",
            source.display(),
            dest.display(),
        ))?;

    // Copy text tags from source to destination via lofty
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
// FLAC encoding (via flac-codec — pure Rust, streaming encoder)
// ============================================================================

fn encode_flac(source: &Path, dest: &Path) -> Result<()> {
    let crate::corpus::codecs::AudioSource {
        mut format, mut decoder, sample_rate, channels, bits_per_sample, ..
    } = crate::corpus::codecs::open_audio_source(source)?;

    // Stream decoded packets directly through the encoder instead of collecting
    // all samples first. The encoder accepts total_samples: None and seeks back
    // at finalize() to update STREAMINFO with the actual count.
    //
    // The old collect-then-write approach doubled memory (~340MB/thread for a
    // typical file) and couldn't recover if disk writes stalled (the entire
    // file's samples trapped in the encoder's internal VecDeque).
    let mut encoder = flac_codec::encode::FlacSampleWriter::create(
        dest,
        flac_codec::encode::Options::default(),
        sample_rate,
        bits_per_sample,
        channels as u8,
        None,
    ).map_err(|e| anyhow::anyhow!("FLAC encoder creation failed: {}", e))?;

    let mut wrote_any = false;
    while let Ok(packet) = format.next_packet() {
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let (samples, _frames) = convert_audio_buffer_to_i32(decoded, channels)?;
                encoder.write(&samples)
                    .map_err(|e| anyhow::anyhow!("FLAC encoding failed: {}", e))?;
                wrote_any = true;
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow::anyhow!("Decode error: {}", e)),
        }
    }

    if !wrote_any {
        return Err(anyhow::anyhow!("No audio frames decoded from source"));
    }

    encoder.finalize()
        .map_err(|e| anyhow::anyhow!("FLAC finalization failed: {}", e))?;

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

    let crate::corpus::codecs::AudioSource {
        mut format, mut decoder, sample_rate, channels, ..
    } = crate::corpus::codecs::open_audio_source(source)?;

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
        AudioBufferRef::S24(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame].inner());
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::U24(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame].inner() as i32 - (1 << 23));
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::U32(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push((buf.chan(ch)[frame] as i64 - (1i64 << 31)) as i32);
                }
            }
            Ok((samples, frames))
        }
        AudioBufferRef::S8(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(buf.chan(ch)[frame] as i32);
                }
            }
            Ok((samples, frames))
        }
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
        AudioBufferRef::S24(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push((buf.chan(ch)[frame].inner() >> 8) as i16);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::U24(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(((buf.chan(ch)[frame].inner() as i32 - (1 << 23)) >> 8) as i16);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::U32(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push(((buf.chan(ch)[frame] as i64 - (1i64 << 31)) >> 16) as i16);
                }
            }
            Ok(samples)
        }
        AudioBufferRef::S8(buf) => {
            let frames = buf.frames();
            let mut samples = Vec::with_capacity(frames * channels);
            for frame in 0..frames {
                for ch in 0..channels {
                    samples.push((buf.chan(ch)[frame] as i16) << 8);
                }
            }
            Ok(samples)
        }
    }
}

/// Copy embedded pictures (album art) from source to destination.
///
/// Reads pictures from any supported source format (MP3/ID3v2, M4A, etc.)
/// and writes them into FLAC/Opus destination files. For FLAC destinations,
/// pictures are stored as PICTURE metadata blocks via lofty's FlacFile API,
/// which ensures they survive the subsequent text tag write step.
///
/// Silently succeeds if the source has no pictures.
fn copy_pictures(source: &Path, dest: &Path) -> Result<()> {
    use lofty::file::TaggedFileExt;
    use lofty::picture::Picture;
    use lofty::probe::Probe;

    // Read pictures from the source file
    let tagged_file = Probe::open(source)
        .with_context(|| format!("Failed to open source for picture reading: {}", source.display()))?
        .read()
        .with_context(|| format!("Failed to read source for pictures: {}", source.display()))?;

    let pictures: Vec<&Picture> = tagged_file
        .tags()
        .iter()
        .flat_map(|tag| tag.pictures())
        .collect();

    if pictures.is_empty() {
        return Ok(());
    }

    let dest_ext = dest
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    match dest_ext.as_str() {
        "flac" => {
            use lofty::config::{ParseOptions, WriteOptions};
            use lofty::file::AudioFile;
            use lofty::ogg::OggPictureStorage;

            let file = File::open(dest)
                .with_context(|| format!("Failed to open dest FLAC for pictures: {}", dest.display()))?;
            let mut reader = std::io::BufReader::new(file);
            let mut flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default())
                .with_context(|| format!("Failed to read dest FLAC: {}", dest.display()))?;

            for pic in &pictures {
                // info=None lets lofty infer PictureInformation from the picture data
                flac.insert_picture((*pic).clone(), None)
                    .with_context(|| "Failed to insert picture into FLAC")?;
            }

            let write_opts = WriteOptions::new().preferred_padding(0);
            flac.save_to_path(dest, write_opts)
                .with_context(|| format!("Failed to save pictures to FLAC: {}", dest.display()))?;
        }
        // Opus pictures are written inside VorbisComments by copy_tags; lofty's
        // generic Tag → OggOpusFile write path handles this. For now, pictures
        // for Opus targets are not copied here (they'd need to be base64-encoded
        // into METADATA_BLOCK_PICTURE vorbis comment fields).
        _ => {}
    }

    Ok(())
}

/// Copy tags from source file to destination file.
///
/// Reads all text tags from the source via TagSet and writes them to the
/// destination using format-specific concrete types (no lofty generic Tag).
fn copy_tags(source: &Path, dest: &Path) -> Result<()> {
    let source_tags = crate::corpus::tags::TagSet::from_file(source)?;
    if source_tags.iter().count() == 0 {
        return Ok(());
    }
    crate::corpus::tags::write_tags_to_file(dest, &source_tags)
}
