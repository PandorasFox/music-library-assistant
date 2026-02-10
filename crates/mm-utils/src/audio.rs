//! Audio file utilities.
//!
//! Canonical list of supported audio formats and detection helpers.

/// Canonical list of audio file extensions supported by MM.
///
/// All modules should reference this constant to ensure consistency.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "m4a", "opus", "wav", "aiff", "aif", "aac", "wma", "ape", "wv",
];

/// Check if a file extension is a supported audio format.
///
/// Comparison is case-insensitive.
pub fn is_audio_extension(ext: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_formats() {
        assert!(is_audio_extension("flac"));
        assert!(is_audio_extension("mp3"));
        assert!(is_audio_extension("ogg"));
        assert!(is_audio_extension("m4a"));
        assert!(is_audio_extension("opus"));
        assert!(is_audio_extension("wav"));
    }

    #[test]
    fn test_case_insensitive() {
        assert!(is_audio_extension("FLAC"));
        assert!(is_audio_extension("Mp3"));
        assert!(is_audio_extension("OGG"));
    }

    #[test]
    fn test_non_audio() {
        assert!(!is_audio_extension("txt"));
        assert!(!is_audio_extension("jpg"));
        assert!(!is_audio_extension("pdf"));
        assert!(!is_audio_extension(""));
    }
}
