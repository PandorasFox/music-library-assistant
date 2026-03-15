//! Tag Mutation Functions
//!
//! TUI-specific query orchestration for loading tags from disk into TagSets.

use std::collections::HashMap;

use mm_meta::db_types::AudioFile;
use mm_ui::tag_set::TagSet;

/// Load TagSets for a batch of audio files via domain query.
///
/// Returns per-file TagSets in the same order as the input audio files.
/// This is TUI-specific because it uses `App::query()`.
pub(crate) fn load_tag_sets_batch(
    audio_files: &[AudioFile],
    app: &mut crate::App,
) -> Vec<TagSet> {
    if audio_files.is_empty() {
        return Vec::new();
    }
    let zone = audio_files[0].entry.zone;
    let inodes: Vec<i64> = audio_files.iter().map(|af| af.inode()).collect();

    let result = app.query(mm_meta::domain_queries::GetFileTagValues {
        inodes: inodes.clone(),
        zone,
    });

    let tag_map: HashMap<i64, Vec<(String, String)>> = result.into_iter().collect();

    audio_files
        .iter()
        .map(|af| {
            let tags = tag_map
                .get(&af.inode())
                .cloned()
                .unwrap_or_default();
            TagSet::from_pairs(tags)
        })
        .collect()
}
