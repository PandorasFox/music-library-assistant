//! Deployment Path Computation
//!
//! Provides path computation for library deployment. The full deployment planning,
//! execution, and stash logic remains disabled pending transaction API integration.
//!
//! ## Available Functions
//!
//! - `compute_deployment_path_with_tags` - Compute target path from track + tags
//! - `compute_deployment_path` - Convenience wrapper (empty tags, fallback paths)
//!
//! ## Disabled Functionality
//!
//! The following is commented out pending integration with the new transaction API:
//! - `DeploymentPlan`, `DeploymentAction`, `LostFileAction` structs
//! - `create_deployment_plan`, `execute_deployment_plan`
//! - `FullDeploymentStatus`, stale detection, conflict detection
//! - Stash operations for leftover files

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::corpus::db::Track;

// ============================================================================
// Path Computation (enabled for health signal computation)
// ============================================================================

/// Sanitize a path component by replacing invalid characters.
fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

/// Compute deployment path from track and tags.
///
/// Returns: `{album_artist}/{album}/{track}. {title}.{ext}`
/// Or: `{album_artist}/{title}.{ext}` for singles (no album)
/// Or: `[no album artist]/...` if missing album_artist
///
/// Tags should be provided as a HashMap with lowercase keys.
/// Recognized tags: `album_artist`, `artist`, `album`, `title`, `track_number`
pub fn compute_deployment_path_with_tags(track: &Track, tags: &HashMap<String, String>) -> PathBuf {
    // Get extension from original path
    let ext = Path::new(&track.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown");

    // Determine album artist (prefer album_artist, fallback to artist)
    let album_artist = tags
        .get("album_artist")
        .or_else(|| tags.get("artist"))
        .map(|s| s.as_str())
        .unwrap_or("[no album artist]");

    if let Some(album) = tags.get("album") {
        // Full path: {album_artist}/{album}/{track}. {title}.{ext}
        let mut path = PathBuf::new();
        path.push(sanitize_path_component(album_artist));
        path.push(sanitize_path_component(album));

        let filename = if let Some(title) = tags.get("title") {
            if let Some(track_num_str) = tags.get("track_number") {
                if let Ok(track_num) = track_num_str.parse::<i32>() {
                    format!(
                        "{:02}. {}.{}",
                        track_num,
                        sanitize_path_component(title),
                        ext
                    )
                } else {
                    format!("{}.{}", sanitize_path_component(title), ext)
                }
            } else {
                format!("{}.{}", sanitize_path_component(title), ext)
            }
        } else {
            // Fallback to original filename
            Path::new(&track.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        };
        path.push(filename);
        path
    } else {
        // Single: {album_artist}/{title}.{ext}
        let mut path = PathBuf::new();
        path.push(sanitize_path_component(album_artist));

        let filename = if let Some(title) = tags.get("title") {
            format!("{}.{}", sanitize_path_component(title), ext)
        } else {
            Path::new(&track.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        };
        path.push(filename);
        path
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(path: &str) -> Track {
        Track {
            id: None,
            path: path.to_string(),
            source: "corpus".to_string(),
            inode: 123,
            file_size: 1000,
            file_type: "flac".to_string(),
            duration_ms: Some(180000),
            bitrate_kbps: None,
            sample_rate: None,
            fingerprint: None,
            needs_disk_flush: false,
        }
    }

    #[test]
    fn test_sanitize_path_component() {
        assert_eq!(sanitize_path_component("Hello/World"), "Hello_World");
        assert_eq!(sanitize_path_component("Track:01"), "Track_01");
        assert_eq!(sanitize_path_component("Normal Name"), "Normal Name");
    }

    #[test]
    fn test_compute_deployment_path_full() {
        let track = make_track("/corpus/test.flac");
        let tags: HashMap<String, String> = [
            ("artist".to_string(), "Artist Name".to_string()),
            ("album".to_string(), "Album Name".to_string()),
            ("album_artist".to_string(), "Album Artist".to_string()),
            ("title".to_string(), "Track Title".to_string()),
            ("track_number".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect();

        let path = compute_deployment_path_with_tags(&track, &tags);
        assert_eq!(
            path,
            PathBuf::from("Album Artist/Album Name/01. Track Title.flac")
        );
    }

    #[test]
    fn test_compute_deployment_path_single() {
        let mut track = make_track("/corpus/test.mp3");
        track.file_type = "mp3".to_string();
        let tags: HashMap<String, String> = [
            ("artist".to_string(), "Artist Name".to_string()),
            ("title".to_string(), "Single Track".to_string()),
        ]
        .into_iter()
        .collect();

        let path = compute_deployment_path_with_tags(&track, &tags);
        assert_eq!(path, PathBuf::from("Artist Name/Single Track.mp3"));
    }

    #[test]
    fn test_compute_deployment_path_no_album_artist() {
        let track = make_track("/corpus/test.flac");
        let tags: HashMap<String, String> = [
            ("album".to_string(), "Album".to_string()),
            ("title".to_string(), "Title".to_string()),
            ("track_number".to_string(), "5".to_string()),
        ]
        .into_iter()
        .collect();

        let path = compute_deployment_path_with_tags(&track, &tags);
        assert_eq!(
            path,
            PathBuf::from("[no album artist]/Album/05. Title.flac")
        );
    }
}

// ============================================================================
// DISABLED: Full Deployment Planning & Execution
// ============================================================================
//
// The following code is disabled pending integration with the new transaction API
// and library health signals. When re-enabling:
// - Replace *_to_decisions() functions with *_to_mutations() returning Vec<Mutation>
// - Integrate with DeriveLibraryHealthSignals computations
// - Wire into the DecisionWitness transaction flow
//
// ```
// use anyhow::Result;
// use std::collections::HashSet;
// use std::os::unix::fs::MetadataExt;
//
// use crate::config::Config;
// use crate::corpus::db::Database;
// use crate::corpus::health::library::{
//     get_configured_library_names, get_deployable_corpus_tracks, walk_library_files,
// };
//
// #[derive(Debug, Clone)]
// pub struct DeploymentPlan {
//     pub library_name: String,
//     pub files_to_deploy: Vec<DeploymentAction>,
//     pub files_already_deployed: Vec<Track>,
//     pub lost_files: Vec<LostFileAction>,
// }
//
// #[derive(Debug, Clone)]
// pub struct DeploymentAction {
//     pub corpus_track: Track,
//     pub target_path: PathBuf,
// }
//
// #[derive(Debug, Clone)]
// pub struct LostFileAction {
//     pub current_path: PathBuf,
//     pub target_path: PathBuf,
//     pub inode: i64,
// }
//
// ... (rest of deployment planning, execution, stash logic)
// ```
