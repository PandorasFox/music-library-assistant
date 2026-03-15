//! Tag Mutation Functions
//!
//! Re-exports pure functions from mm-ui and provides TUI-specific
//! query orchestration (load_tag_fields_batch).

use std::collections::HashMap;

use mm_meta::db_types::AudioFile;

use mm_ui::domain_types::TagField;

// Re-export pure functions from mm-ui for internal callers.
pub use mm_ui::tag_mutations::{
    aggregate_tags_from_fields,
    changes_to_mutations,
    compute_changes,
    tag_pairs_to_tag_fields,
};

/// Load tag fields for a batch of audio files via domain query.
///
/// Returns per-file tag fields in the same order as the input audio files.
/// This is TUI-specific because it uses `App::query()`.
pub(crate) fn load_tag_fields_batch(
    audio_files: &[AudioFile],
    app: &mut crate::App,
) -> Vec<Vec<TagField>> {
    if audio_files.is_empty() {
        return Vec::new();
    }
    let zone = audio_files[0].entry.zone;
    let inodes: Vec<i64> = audio_files.iter().map(|af| af.inode()).collect();

    let result = app.query(mm_meta::domain_queries::GetFileTagValues { inodes: inodes.clone(), zone });

    let tag_map: HashMap<i64, Vec<(String, String)>> = result.into_iter().collect();

    audio_files
        .iter()
        .map(|af| {
            let tags = tag_map
                .get(&af.inode())
                .cloned()
                .unwrap_or_default();
            tag_pairs_to_tag_fields(tags)
        })
        .collect()
}
