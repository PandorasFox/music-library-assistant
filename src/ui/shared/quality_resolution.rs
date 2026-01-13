//! Quality-Based Resolution Options
//!
//! Shared components for quality-based resolution across canonicalization flows.
//! Provides "stash lower bitrates" and "stash by filetype" options that can be
//! integrated into any flow dealing with duplicate or variant tracks.
//!
//! ## Integration Points
//! - Artist canonicalization: After squashing, detect quality variants
//! - Album artist canonicalization: Same
//! - Album canonicalization: Especially for EP/album variant detection
//! - Fingerprint deduplication: Already has quality comparison

use std::collections::{HashMap, HashSet};

use crate::corpus::db::types::Track;
use crate::corpus::db::{ChangeStatus, ChangeType, PendingChange};

/// Quality tier for audio formats.
/// Higher tier = higher quality (lossless beats lossy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QualityTier {
    /// Legacy lossy formats: MP3, WMA
    LegacyLossy = 1,
    /// High-quality lossy: OGG, OPUS, M4A, AAC
    HighQualityLossy = 2,
    /// Lossless formats: FLAC, WAV, APE, WV, ALAC
    Lossless = 3,
}

impl QualityTier {
    /// Get tier for a file type string.
    pub fn from_file_type(file_type: &str) -> Self {
        match file_type.to_lowercase().as_str() {
            "flac" | "wav" | "ape" | "wv" | "alac" | "aiff" => Self::Lossless,
            "ogg" | "opus" | "m4a" | "aac" => Self::HighQualityLossy,
            "mp3" | "wma" => Self::LegacyLossy,
            _ => Self::LegacyLossy, // Default to lowest for unknown
        }
    }

    /// Display name for the tier.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Lossless => "Lossless",
            Self::HighQualityLossy => "High-Quality Lossy",
            Self::LegacyLossy => "Legacy Lossy",
        }
    }
}

/// Quality analysis result for a set of tracks.
#[derive(Debug, Clone)]
pub struct QualityAnalysis {
    /// All unique file types found
    pub file_types: HashSet<String>,
    /// All unique bitrates found (sorted descending)
    pub bitrates: Vec<i32>,
    /// Tracks grouped by file type
    pub tracks_by_type: HashMap<String, Vec<TrackQualityInfo>>,
    /// Tracks grouped by bitrate tier
    pub tracks_by_bitrate_tier: HashMap<BitrateTier, Vec<TrackQualityInfo>>,
    /// Maximum bitrate found
    pub max_bitrate: Option<i32>,
    /// Minimum bitrate found
    pub min_bitrate: Option<i32>,
    /// Whether there are quality differences worth resolving
    pub has_quality_differences: bool,
}

/// Bitrate tier for grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BitrateTier {
    /// Below 128 kbps
    VeryLow,
    /// 128-192 kbps
    Low,
    /// 192-256 kbps
    Medium,
    /// 256-320 kbps
    High,
    /// Above 320 kbps (typically lossless)
    VeryHigh,
}

impl BitrateTier {
    /// Get tier for a bitrate value.
    pub fn from_bitrate(kbps: i32) -> Self {
        match kbps {
            0..=127 => Self::VeryLow,
            128..=191 => Self::Low,
            192..=255 => Self::Medium,
            256..=320 => Self::High,
            _ => Self::VeryHigh,
        }
    }

    /// Display name for the tier.
    pub fn name(&self) -> &'static str {
        match self {
            Self::VeryLow => "<128 kbps",
            Self::Low => "128-192 kbps",
            Self::Medium => "192-256 kbps",
            Self::High => "256-320 kbps",
            Self::VeryHigh => ">320 kbps",
        }
    }
}

/// Simplified track info for quality analysis.
#[derive(Debug, Clone)]
pub struct TrackQualityInfo {
    pub track_id: i64,
    pub path: String,
    pub file_type: String,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub quality_tier: QualityTier,
}

impl From<&Track> for TrackQualityInfo {
    fn from(track: &Track) -> Self {
        Self {
            track_id: track.id.unwrap_or(0),
            path: track.path.clone(),
            file_type: track.file_type.clone(),
            bitrate_kbps: track.bitrate_kbps,
            sample_rate: track.sample_rate,
            quality_tier: QualityTier::from_file_type(&track.file_type),
        }
    }
}

/// Quality resolution option that can be offered to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualityResolutionOption {
    /// Stash all tracks with bitrate below the maximum
    StashLowerBitrates {
        /// Tracks that would be stashed
        track_count: usize,
        /// The threshold bitrate (max found)
        threshold: i32,
    },
    /// Stash all tracks of a specific file type
    StashFileType {
        file_type: String,
        track_count: usize,
        /// Quality tier of this file type
        tier: QualityTier,
    },
    /// Stash tracks below a specific bitrate tier
    StashBelowTier {
        tier: BitrateTier,
        track_count: usize,
    },
}

impl QualityResolutionOption {
    /// Get display label for this option.
    pub fn label(&self) -> String {
        match self {
            Self::StashLowerBitrates { track_count, threshold } => {
                format!("Stash lower bitrates (<{} kbps) [{} tracks]", threshold, track_count)
            }
            Self::StashFileType { file_type, track_count, .. } => {
                format!("Stash {} [{} tracks]", file_type.to_uppercase(), track_count)
            }
            Self::StashBelowTier { tier, track_count } => {
                format!("Stash {} [{} tracks]", tier.name(), track_count)
            }
        }
    }

    /// Get short label for button display.
    pub fn short_label(&self) -> String {
        match self {
            Self::StashLowerBitrates { .. } => "Stash lower bitrates".to_string(),
            Self::StashFileType { file_type, .. } => format!("Stash {}", file_type.to_uppercase()),
            Self::StashBelowTier { tier, .. } => format!("Stash {}", tier.name()),
        }
    }
}

/// Analyze a set of tracks for quality differences.
pub fn analyze_quality(tracks: &[Track]) -> QualityAnalysis {
    let mut file_types = HashSet::new();
    let mut bitrates = Vec::new();
    let mut tracks_by_type: HashMap<String, Vec<TrackQualityInfo>> = HashMap::new();
    let mut tracks_by_bitrate_tier: HashMap<BitrateTier, Vec<TrackQualityInfo>> = HashMap::new();

    for track in tracks {
        let info = TrackQualityInfo::from(track);
        let file_type_lower = track.file_type.to_lowercase();

        file_types.insert(file_type_lower.clone());
        tracks_by_type.entry(file_type_lower).or_default().push(info.clone());

        if let Some(kbps) = track.bitrate_kbps {
            bitrates.push(kbps);
            let tier = BitrateTier::from_bitrate(kbps);
            tracks_by_bitrate_tier.entry(tier).or_default().push(info);
        }
    }

    bitrates.sort_by(|a, b| b.cmp(a)); // Descending
    bitrates.dedup();

    let max_bitrate = bitrates.first().copied();
    let min_bitrate = bitrates.last().copied();

    // Quality differences exist if:
    // - Multiple file types with different tiers
    // - Or significant bitrate spread (>30% difference)
    let has_quality_differences = file_types.len() > 1 || {
        match (max_bitrate, min_bitrate) {
            (Some(max), Some(min)) if max > 0 => {
                let ratio = min as f64 / max as f64;
                ratio < 0.7 // More than 30% difference
            }
            _ => false,
        }
    };

    QualityAnalysis {
        file_types,
        bitrates,
        tracks_by_type,
        tracks_by_bitrate_tier,
        max_bitrate,
        min_bitrate,
        has_quality_differences,
    }
}

/// Generate quality resolution options based on analysis.
pub fn get_resolution_options(analysis: &QualityAnalysis) -> Vec<QualityResolutionOption> {
    let mut options = Vec::new();

    // Option 1: Stash lower bitrates (if significant spread exists)
    if let (Some(max), Some(min)) = (analysis.max_bitrate, analysis.min_bitrate) {
        if max > min && (min as f64 / max as f64) < 0.9 {
            let lower_count: usize = analysis.tracks_by_bitrate_tier.iter()
                .filter(|(tier, _)| {
                    BitrateTier::from_bitrate(max) > **tier
                })
                .map(|(_, tracks)| tracks.len())
                .sum();

            if lower_count > 0 {
                options.push(QualityResolutionOption::StashLowerBitrates {
                    track_count: lower_count,
                    threshold: max,
                });
            }
        }
    }

    // Option 2: Stash by file type (if multiple types exist)
    if analysis.file_types.len() > 1 {
        // Sort by quality tier (lowest first - those are stash candidates)
        let mut type_options: Vec<_> = analysis.tracks_by_type.iter()
            .map(|(file_type, tracks)| {
                let tier = QualityTier::from_file_type(file_type);
                (file_type.clone(), tracks.len(), tier)
            })
            .collect();

        type_options.sort_by_key(|(_, _, tier)| *tier);

        // Offer to stash each type except the highest quality one
        for (file_type, count, tier) in type_options.iter().take(type_options.len().saturating_sub(1)) {
            options.push(QualityResolutionOption::StashFileType {
                file_type: file_type.clone(),
                track_count: *count,
                tier: *tier,
            });
        }
    }

    options
}

/// Generate stash mutations for a quality resolution decision.
pub fn generate_stash_changes(
    option: &QualityResolutionOption,
    analysis: &QualityAnalysis,
    session_id: &str,
    stash_root: &std::path::Path,
    stash_subdir: &str,
) -> Vec<PendingChange> {
    let mut changes = Vec::new();

    let tracks_to_stash: Vec<&TrackQualityInfo> = match option {
        QualityResolutionOption::StashLowerBitrates { threshold, .. } => {
            analysis.tracks_by_bitrate_tier.iter()
                .filter(|(tier, _)| BitrateTier::from_bitrate(*threshold) > **tier)
                .flat_map(|(_, tracks)| tracks.iter())
                .collect()
        }
        QualityResolutionOption::StashFileType { file_type, .. } => {
            analysis.tracks_by_type.get(file_type)
                .map(|tracks| tracks.iter().collect())
                .unwrap_or_default()
        }
        QualityResolutionOption::StashBelowTier { tier, .. } => {
            analysis.tracks_by_bitrate_tier.iter()
                .filter(|(t, _)| *t < tier)
                .flat_map(|(_, tracks)| tracks.iter())
                .collect()
        }
    };

    for track in tracks_to_stash {
        // Generate target path in stash
        let source_path = std::path::Path::new(&track.path);
        let file_name = source_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let target_path = stash_root
            .join(stash_subdir)
            .join(file_name);

        changes.push(PendingChange {
            id: None,
            session_id: session_id.to_string(),
            change_type: ChangeType::Delete, // Delete = move to stash
            source_path: track.path.clone(),
            target_path: Some(target_path.to_string_lossy().to_string()),
            metadata_changes: Some(serde_json::json!({
                "reason": "quality_resolution",
                "option": option.short_label(),
                "original_bitrate": track.bitrate_kbps,
                "original_type": track.file_type,
            }).to_string()),
            created_at: None,
            status: ChangeStatus::Pending,
        });
    }

    changes
}

/// UI state for quality resolution toggles.
#[derive(Debug, Clone)]
pub struct QualityResolutionState {
    /// Available resolution options
    pub options: Vec<QualityResolutionOption>,
    /// Which options are currently selected (toggled on)
    pub selected: HashSet<usize>,
    /// Current cursor position
    pub cursor: usize,
}

impl QualityResolutionState {
    /// Create a new state from analysis.
    pub fn new(analysis: &QualityAnalysis) -> Self {
        let options = get_resolution_options(analysis);
        Self {
            options,
            selected: HashSet::new(),
            cursor: 0,
        }
    }

    /// Check if any options are available.
    pub fn has_options(&self) -> bool {
        !self.options.is_empty()
    }

    /// Toggle the current option.
    pub fn toggle_current(&mut self) {
        if self.cursor < self.options.len() {
            if self.selected.contains(&self.cursor) {
                self.selected.remove(&self.cursor);
            } else {
                self.selected.insert(self.cursor);
            }
        }
    }

    /// Move cursor up.
    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.options.len() {
            self.cursor += 1;
        }
    }

    /// Get selected options.
    pub fn get_selected_options(&self) -> Vec<&QualityResolutionOption> {
        self.selected.iter()
            .filter_map(|&idx| self.options.get(idx))
            .collect()
    }

    /// Check if an option is selected.
    pub fn is_selected(&self, idx: usize) -> bool {
        self.selected.contains(&idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(id: i64, file_type: &str, bitrate: Option<i32>) -> Track {
        Track {
            id: Some(id),
            path: format!("/test/track_{}.{}", id, file_type),
            source: "corpus".to_string(),
            inode: id,
            file_size: 1000,
            file_type: file_type.to_string(),
            artist: Some("Test Artist".to_string()),
            album: Some("Test Album".to_string()),
            album_artist: None,
            title: Some(format!("Track {}", id)),
            track_number: Some(id as i32),
            genre: None,
            duration_ms: Some(180000),
            bitrate_kbps: bitrate,
            sample_rate: Some(44100),
            fingerprint: None,
            isrc: None,
        }
    }

    #[test]
    fn test_quality_tier_ordering() {
        assert!(QualityTier::Lossless > QualityTier::HighQualityLossy);
        assert!(QualityTier::HighQualityLossy > QualityTier::LegacyLossy);
    }

    #[test]
    fn test_quality_analysis_mixed_formats() {
        let tracks = vec![
            make_track(1, "flac", Some(1000)),
            make_track(2, "mp3", Some(320)),
            make_track(3, "mp3", Some(128)),
        ];

        let analysis = analyze_quality(&tracks);

        assert!(analysis.has_quality_differences);
        assert_eq!(analysis.file_types.len(), 2);
        assert_eq!(analysis.max_bitrate, Some(1000));
        assert_eq!(analysis.min_bitrate, Some(128));
    }

    #[test]
    fn test_resolution_options_generation() {
        let tracks = vec![
            make_track(1, "flac", Some(1000)),
            make_track(2, "mp3", Some(320)),
            make_track(3, "mp3", Some(128)),
        ];

        let analysis = analyze_quality(&tracks);
        let options = get_resolution_options(&analysis);

        // Should have options for stashing lower bitrates and mp3 files
        assert!(!options.is_empty());
    }
}
