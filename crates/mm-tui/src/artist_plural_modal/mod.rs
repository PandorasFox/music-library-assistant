//! Artist Plural Normalization Modal
//!
//! Provides the interactive workflow for resolving multi-valued ARTIST/ALBUMARTIST
//! tags by pluralizing them (semicolon-joined singular + individual plural tags).
//!
//! Data wrapper, buttons, and actions live in `mm_ui::resolutions::artist_plural`.
//! This module provides the ratatui-specific `ModalFrame` impl.

mod preview;
pub mod types;

pub use types::{
    ArtistPluralAction, ArtistPluralData, ArtistPluralState,
};
