//! Native audio transcoding.
//!
//! Decodes audio via symphonia and re-encodes to FLAC (via flac-codec, pure Rust).
//! No external subprocess required.

use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;

use crate::witch::MutationExecutionWitness;

use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::Signal;

// TranscodeTarget is defined in mm-meta; re-exported here for compatibility.
pub use mm_meta::transcode::TranscodeTarget;

/// Transcode a source audio file to the target format using native decode/encode.
///
/// The destination path must not already exist.
pub fn transcode(
    source: &Path,
    dest: &Path,
    target: TranscodeTarget,
    _witness: &MutationExecutionWitness,
) -> Result<()> {
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
        TranscodeTarget::Flac => {
            encode_flac(source, dest)?;
        }
    }

    // Copy pictures (album art) from source into destination before text tags.
    // For FLAC destinations, these become PICTURE metadata blocks that survive
    // the subsequent text tag write (lofty reads them into FlacFile.pictures on
    // parse and chains them back into output on save).
    copy_pictures(source, dest).with_context(|| {
        format!(
            "Transcode succeeded but picture copy failed: {} -> {}",
            source.display(),
            dest.display(),
        )
    })?;

    // Copy text tags from source to destination via lofty
    copy_tags(source, dest).with_context(|| {
        format!(
            "Transcode succeeded but tag copy failed: {} -> {}",
            source.display(),
            dest.display()
        )
    })?;

    // Verify the output file was created
    if !dest.exists() {
        return Err(anyhow::anyhow!(
            "Transcode completed but output file was not created: {}",
            dest.display()
        ));
    }

    // Validate the encoder produced a decodable file. flac_codec's
    // `finalize()` returns Ok when the file *closes cleanly*, not when its
    // frame bitstream is decodable; a buggy encoder run can leave well-formed
    // metadata blocks wrapped around corrupt frames. Reusing the same audio
    // integrity check that VerifyAudio runs in the observation phase keeps
    // strictness consistent: anything we'd later flag as CorruptFile gets
    // caught here, before the source is stashed.
    crate::corpus::metadata::verify_audio_integrity(dest).with_context(|| {
        format!(
            "Transcode produced an undecodable file: {} -> {}",
            source.display(),
            dest.display()
        )
    })?;

    Ok(())
}

// ============================================================================
// FLAC encoding (via flac-codec — pure Rust, streaming encoder)
// ============================================================================

fn encode_flac(source: &Path, dest: &Path) -> Result<()> {
    let crate::corpus::codecs::AudioSource {
        mut format,
        mut decoder,
        sample_rate,
        channels,
        bits_per_sample,
        ..
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
    )
    .map_err(|e| anyhow::anyhow!("FLAC encoder creation failed: {}", e))?;

    let mut wrote_any = false;
    while let Ok(packet) = format.next_packet() {
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let (samples, _frames) = convert_audio_buffer_to_i32(decoded, channels)?;
                encoder
                    .write(&samples)
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

    encoder
        .finalize()
        .map_err(|e| anyhow::anyhow!("FLAC finalization failed: {}", e))?;

    Ok(())
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

/// Copy embedded pictures (album art) from source to destination.
///
/// Reads pictures from any supported source format (MP3/ID3v2, M4A, etc.)
/// and writes them into the FLAC destination file as PICTURE metadata blocks
/// via lofty's FlacFile API, which ensures they survive the subsequent text
/// tag write step.
///
/// Silently succeeds if the source has no pictures.
fn copy_pictures(source: &Path, dest: &Path) -> Result<()> {
    use lofty::file::TaggedFileExt;
    use lofty::picture::Picture;
    use lofty::probe::Probe;

    // Read pictures from the source file
    let tagged_file = Probe::open(source)
        .with_context(|| {
            format!(
                "Failed to open source for picture reading: {}",
                source.display()
            )
        })?
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

    if dest_ext == "flac" {
        use lofty::config::{ParseOptions, WriteOptions};
        use lofty::file::AudioFile;
        use lofty::ogg::OggPictureStorage;

        let file = File::open(dest).with_context(|| {
            format!("Failed to open dest FLAC for pictures: {}", dest.display())
        })?;
        let mut reader = std::io::BufReader::new(file);
        let mut flac = lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default())
            .with_context(|| format!("Failed to read dest FLAC: {}", dest.display()))?;

        for pic in &pictures {
            // info=None lets lofty infer PictureInformation from the picture data
            flac.insert_picture((*pic).clone(), None)
                .with_context(|| "Failed to insert picture into FLAC")?;
        }

        flac.save_to_path(dest, WriteOptions::default())
            .with_context(|| format!("Failed to save pictures to FLAC: {}", dest.display()))?;
    }

    Ok(())
}

/// Copy tags from source file to destination file.
///
/// Reads all text tags from the source via TagSet and writes them to the
/// destination using format-specific concrete types (no lofty generic Tag).
fn copy_tags(source: &Path, dest: &Path) -> Result<()> {
    let source_tags = crate::corpus::tags::from_file(source)?;
    if source_tags.iter().count() == 0 {
        return Ok(());
    }
    crate::corpus::tags::write_tags_to_file(dest, &source_tags)
}
