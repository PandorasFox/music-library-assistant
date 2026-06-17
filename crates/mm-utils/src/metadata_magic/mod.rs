//! Metadata Magic - Tag Normalization and Comparison
//!
//! Generic corpus metadata-munging operations for fuzzy matching and
//! canonicalization of music metadata tags.
//!
//! ## Modules
//!
//! - `artist` - Artist and album_artist name normalization
//! - `album` - Album name normalization with EP/LP and edition handling
//! - `genre` - Genre normalization with compound genre unification

pub mod album;
pub mod artist;
pub mod genre;

// Re-export commonly used items
pub use album::{
    albums_equivalent, edition_rank, is_superior_edition, normalize_album, AlbumFormat,
    NormalizedAlbum,
};
pub use artist::{article_sort_form, normalize_album_artist, normalize_artist};
pub use genre::normalize_genre;
