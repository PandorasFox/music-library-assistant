//! Codec Registry & Audio Source Opening
//!
//! Extends symphonia's default codecs with additional format support, and provides
//! a shared `open_audio_source()` for reliably opening audio files — including
//! formats like AAC-in-M4A where container metadata may omit channel/sample-rate
//! info that lives in codec-specific headers instead.
//!
//! ## TODO: Remove codec registry when symphonia adds native opus support
//!
//! The custom registry exists because symphonia 0.5.x doesn't include an opus decoder.
//! Once symphonia ships native opus support, `get_codecs()`/`make_decoder()` can be
//! deleted and all call sites can revert to `symphonia::default::get_codecs()`.
//!
//! Tracking: https://github.com/pdeljanov/Symphonia/issues/8

use std::sync::OnceLock;
use symphonia_core::codecs::{CodecRegistry, Decoder, DecoderOptions, CodecParameters};
use symphonia_core::errors::Result;

/// Global codec registry with opus support.
///
/// Lazily initialized on first access. Thread-safe.
static CODEC_REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();

/// Get the custom codec registry with opus support.
///
/// ## TODO: Remove when symphonia adds native opus support
///
/// Replace all calls to this function with `symphonia::default::get_codecs()`
/// once symphonia includes native opus decoding.
pub fn get_codecs() -> &'static CodecRegistry {
    CODEC_REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();

        // Register all default symphonia codecs
        // (This duplicates what symphonia::default::get_codecs() provides)
        symphonia::default::register_enabled_codecs(&mut registry);

        // Register opus via libopus adapter
        // TODO: Remove this line when symphonia adds native opus support
        registry.register_all::<symphonia_adapter_libopus::OpusDecoder>();

        registry
    })
}

/// Create a decoder for the given codec parameters.
///
/// Convenience wrapper that uses our extended codec registry.
///
/// ## TODO: Remove when symphonia adds native opus support
///
/// Replace with direct calls to `symphonia::default::get_codecs().make(...)`
pub fn make_decoder(
    params: &CodecParameters,
    options: &DecoderOptions,
) -> Result<Box<dyn Decoder>> {
    get_codecs().make(params, options)
}

// ============================================================================
// Audio source opening with spec discovery
// ============================================================================

/// An opened audio source ready for decoding.
///
/// Wraps a symphonia format reader + decoder with guaranteed audio spec
/// (sample rate, channels, bits per sample), even for formats where the
/// container metadata doesn't include them (e.g. AAC-in-M4A).
pub struct AudioSource {
    pub format: Box<dyn symphonia::core::formats::FormatReader>,
    pub decoder: Box<dyn Decoder>,
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: u32,
}

/// Open an audio file, probe its format, create a decoder, and discover audio spec.
///
/// For most formats (FLAC, MP3, OGG, WAV, etc.) the container metadata includes
/// channel layout and sample rate directly in the track's `codec_params`.
///
/// For AAC-in-M4A the audio spec lives in the codec's AudioSpecificConfig rather
/// than the MP4 track header, so `codec_params.channels` and sometimes
/// `codec_params.sample_rate` are `None`. In that case this function decodes the
/// first packet to discover the actual spec from the decoded audio buffer, then
/// seeks back to the beginning so callers get the full stream.
pub fn open_audio_source(path: &std::path::Path) -> anyhow::Result<AudioSource> {
    use anyhow::Context;
    use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open source: {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        if let Some(ext_str) = ext.to_str() {
            hint.with_extension(ext_str);
        }
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .with_context(|| format!("Failed to probe audio: {}", path.display()))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .context("No default audio track found")?;

    let sample_rate_opt = track.codec_params.sample_rate;
    let channels_opt = track.codec_params.channels.map(|ch| ch.count());
    // Default to 16 bits if not specified (common for lossy decoders)
    let bits_per_sample = track.codec_params.bits_per_sample.unwrap_or(16);
    let track_id = track.id;

    let mut decoder = make_decoder(&track.codec_params, &Default::default())
        .context("Failed to create decoder")?;

    let (sample_rate, channels) = match (sample_rate_opt, channels_opt) {
        (Some(sr), Some(ch)) => (sr, ch),
        _ => {
            // Container metadata incomplete — decode first packet to discover
            // the actual audio spec from the decoder output.
            let packet = format.next_packet()
                .context("No packets in source to determine audio spec")?;
            let decoded = decoder.decode(&packet)
                .context("Failed to decode first packet for audio spec discovery")?;
            let spec = decoded.spec();
            let sr = sample_rate_opt.unwrap_or(spec.rate);
            let ch = channels_opt.unwrap_or_else(|| spec.channels.count());

            // Seek back to start so callers get the full stream
            format.seek(SeekMode::Coarse, SeekTo::TimeStamp { ts: 0, track_id })
                .context("Failed to seek back after audio spec discovery")?;
            decoder.reset();

            (sr, ch)
        }
    };

    Ok(AudioSource {
        format,
        decoder,
        sample_rate,
        channels,
        bits_per_sample,
    })
}
