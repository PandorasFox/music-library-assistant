//! Tag Cloud: Cached corpus tag values for collision detection.
//!
//! The TagCloud is built once per operational flow (scan, eyeballing, triage)
//! and provides efficient collision detection without repeated database queries.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::normalization::{normalize_album, normalize_album_artist, normalize_artist, normalize_genre};
use crate::config;
use crate::corpus::db::Database;

/// Cached corpus tag values for efficient collision detection.
/// Built once per operational flow, invalidated after mutations.
#[derive(Debug, Clone)]
pub struct TagCloud {
    /// normalized_key -> (original_values with counts)
    artists: HashMap<String, HashMap<String, usize>>,
    /// normalized_key -> (original_values with counts)
    album_artists: HashMap<String, HashMap<String, usize>>,
    /// (normalized_artist, normalized_album) -> (original_album_values with counts)
    albums: HashMap<(String, String), HashMap<String, usize>>,
    /// normalized_key -> (original_values with counts)
    genres: HashMap<String, HashMap<String, usize>>,
    /// When this cloud was built
    built_at: Instant,
    /// Track count when built (for staleness detection)
    track_count: usize,
}

impl TagCloud {
    /// Build from database (called once per operational flow).
    /// This is an expensive operation - should be run in background thread.
    pub fn build(db: &Database) -> Result<Self> {
        let start = Instant::now();

        let tracks = db.get_all_tracks(Some("corpus"))?;
        let track_count = tracks.len();

        let mut artists: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let mut album_artists: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let mut albums: HashMap<(String, String), HashMap<String, usize>> = HashMap::new();
        let mut genres: HashMap<String, HashMap<String, usize>> = HashMap::new();

        for track in &tracks {
            // Artist
            if let Some(artist) = &track.artist {
                if !artist.is_empty() {
                    let normalized = normalize_artist(artist);
                    artists
                        .entry(normalized)
                        .or_default()
                        .entry(artist.clone())
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                }
            }

            // Album Artist
            if let Some(album_artist) = &track.album_artist {
                if !album_artist.is_empty() {
                    let normalized = normalize_album_artist(album_artist);
                    album_artists
                        .entry(normalized)
                        .or_default()
                        .entry(album_artist.clone())
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                }
            }

            // Album (keyed by artist context)
            if let Some(album) = &track.album {
                if !album.is_empty() {
                    // Use album_artist if available, otherwise fall back to artist
                    let artist_context = track
                        .album_artist
                        .as_ref()
                        .or(track.artist.as_ref())
                        .map(|a| normalize_artist(a))
                        .unwrap_or_default();

                    let normalized_album = normalize_album(album);
                    let key = (artist_context, normalized_album);

                    albums
                        .entry(key)
                        .or_default()
                        .entry(album.clone())
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                }
            }

            // Genre
            if let Some(genre) = &track.genre {
                if !genre.is_empty() {
                    let normalized = normalize_genre(genre);
                    genres
                        .entry(normalized)
                        .or_default()
                        .entry(genre.clone())
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                }
            }
        }

        let duration = start.elapsed();
        let _ = config::log_message(&format!(
            "TagCloud built: {} tracks, {} artists, {} album_artists, {} albums, {} genres in {:?}",
            track_count,
            artists.len(),
            album_artists.len(),
            albums.len(),
            genres.len(),
            duration
        ));

        Ok(TagCloud {
            artists,
            album_artists,
            albums,
            genres,
            built_at: Instant::now(),
            track_count,
        })
    }

    /// Check if cache is stale (older than max_age).
    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.built_at.elapsed() > max_age
    }

    /// Get when this cloud was built.
    pub fn built_at(&self) -> Instant {
        self.built_at
    }

    /// Get track count when built.
    pub fn track_count(&self) -> usize {
        self.track_count
    }

    // ========================================================================
    // Artist accessors
    // ========================================================================

    /// Get all normalized artist keys.
    pub fn artist_keys(&self) -> impl Iterator<Item = &String> {
        self.artists.keys()
    }

    /// Get all original values for a normalized artist key.
    pub fn artist_variants(&self, normalized_key: &str) -> Option<&HashMap<String, usize>> {
        self.artists.get(normalized_key)
    }

    /// Check if a normalized artist key has multiple variants (collision).
    pub fn has_artist_collision(&self, normalized_key: &str) -> bool {
        self.artists
            .get(normalized_key)
            .map(|v| v.len() > 1)
            .unwrap_or(false)
    }

    // ========================================================================
    // Album Artist accessors
    // ========================================================================

    /// Get all normalized album_artist keys.
    pub fn album_artist_keys(&self) -> impl Iterator<Item = &String> {
        self.album_artists.keys()
    }

    /// Get all original values for a normalized album_artist key.
    pub fn album_artist_variants(&self, normalized_key: &str) -> Option<&HashMap<String, usize>> {
        self.album_artists.get(normalized_key)
    }

    /// Check if a normalized album_artist key has multiple variants (collision).
    pub fn has_album_artist_collision(&self, normalized_key: &str) -> bool {
        self.album_artists
            .get(normalized_key)
            .map(|v| v.len() > 1)
            .unwrap_or(false)
    }

    // ========================================================================
    // Album accessors (context-aware)
    // ========================================================================

    /// Get all (artist_context, album) keys.
    pub fn album_keys(&self) -> impl Iterator<Item = &(String, String)> {
        self.albums.keys()
    }

    /// Get all original album values for a (normalized_artist, normalized_album) key.
    pub fn album_variants(
        &self,
        artist_context: &str,
        normalized_album: &str,
    ) -> Option<&HashMap<String, usize>> {
        self.albums.get(&(artist_context.to_string(), normalized_album.to_string()))
    }

    /// Check if an album within an artist context has multiple variants (collision).
    pub fn has_album_collision(&self, artist_context: &str, normalized_album: &str) -> bool {
        self.albums
            .get(&(artist_context.to_string(), normalized_album.to_string()))
            .map(|v| v.len() > 1)
            .unwrap_or(false)
    }

    // ========================================================================
    // Genre accessors
    // ========================================================================

    /// Get all normalized genre keys.
    pub fn genre_keys(&self) -> impl Iterator<Item = &String> {
        self.genres.keys()
    }

    /// Get all original values for a normalized genre key.
    pub fn genre_variants(&self, normalized_key: &str) -> Option<&HashMap<String, usize>> {
        self.genres.get(normalized_key)
    }

    /// Check if a normalized genre key has multiple variants (collision).
    pub fn has_genre_collision(&self, normalized_key: &str) -> bool {
        self.genres
            .get(normalized_key)
            .map(|v| v.len() > 1)
            .unwrap_or(false)
    }

    // ========================================================================
    // Collision summary
    // ========================================================================

    /// Get count of artist collisions (normalized keys with multiple variants).
    pub fn artist_collision_count(&self) -> usize {
        self.artists.values().filter(|v| v.len() > 1).count()
    }

    /// Get count of album_artist collisions.
    pub fn album_artist_collision_count(&self) -> usize {
        self.album_artists.values().filter(|v| v.len() > 1).count()
    }

    /// Get count of album collisions.
    pub fn album_collision_count(&self) -> usize {
        self.albums.values().filter(|v| v.len() > 1).count()
    }

    /// Get count of genre collisions.
    pub fn genre_collision_count(&self) -> usize {
        self.genres.values().filter(|v| v.len() > 1).count()
    }

    /// Create a TagCloud with specific test data.
    #[cfg(test)]
    pub fn new_test(
        artists: HashMap<String, HashMap<String, usize>>,
        album_artists: HashMap<String, HashMap<String, usize>>,
        albums: HashMap<(String, String), HashMap<String, usize>>,
        genres: HashMap<String, HashMap<String, usize>>,
        track_count: usize,
    ) -> Self {
        TagCloud {
            artists,
            album_artists,
            albums,
            genres,
            built_at: Instant::now(),
            track_count,
        }
    }
}

// ============================================================================
// Background Operation
// ============================================================================

/// Spawn a background thread to build the tag cloud.
/// Returns a receiver that will contain the built cloud.
pub fn spawn_tag_cloud_build() -> mpsc::Receiver<Result<TagCloud>> {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let result = (|| {
            let db_path = config::get_db_path()?;
            let db = Database::open(&db_path)?;
            TagCloud::build(&db)
        })();
        let _ = tx.send(result);
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    // Basic tests would require a database, so we just test the collision detection logic
    #[test]
    fn test_collision_counts() {
        let mut artists: HashMap<String, HashMap<String, usize>> = HashMap::new();

        // No collision - single variant
        let mut single = HashMap::new();
        single.insert("Artist A".to_string(), 5);
        artists.insert("artist a".to_string(), single);

        // Collision - multiple variants
        let mut multiple = HashMap::new();
        multiple.insert("nervous_testpilot".to_string(), 3);
        multiple.insert("Nervous Testpilot".to_string(), 7);
        artists.insert("nervous testpilot".to_string(), multiple);

        let cloud = TagCloud {
            artists,
            album_artists: HashMap::new(),
            albums: HashMap::new(),
            genres: HashMap::new(),
            built_at: Instant::now(),
            track_count: 15,
        };

        assert_eq!(cloud.artist_collision_count(), 1);
        assert!(cloud.has_artist_collision("nervous testpilot"));
        assert!(!cloud.has_artist_collision("artist a"));
    }
}
