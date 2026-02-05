//! Custom Codec Registry
//!
//! Extends symphonia's default codecs with additional format support.
//!
//! ## TODO: Remove when symphonia adds native opus support
//!
//! This module exists because symphonia 0.5.x doesn't include an opus decoder.
//! Once symphonia ships native opus support (expected 2025), this entire module
//! can be deleted and all call sites can revert to `symphonia::default::get_codecs()`.
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
