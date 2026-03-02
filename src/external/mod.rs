//! External API clients for metadata lookup.
//!
//! Contains HTTP clients for external services (AcoustID, etc.)
//! used by the fetch thread for background metadata enrichment.

pub mod acoustid;
pub mod musicbrainz;
