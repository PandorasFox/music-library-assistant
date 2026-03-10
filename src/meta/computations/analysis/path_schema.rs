//! Path-tag schema mismatch detection.
//!
//! Compares corpus file paths against configured path-tag schemas,
//! emitting PathTagMismatch signals for files whose path disagrees
//! with their DB tags.

use std::path::Path;

use crate::config::path_schema::PathSchemaMatchResult;
use crate::db::types::Zone;
use crate::db::write_thread;
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_corpus_signals, ComputedCorpusSignal};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    PathMismatchKind, PathTagMismatchData, PathTagMismatchSignal, PathTagValueMismatch};
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

/// Execute DetectPathTagMismatches — compare file paths against configured schemas.
pub fn execute_detect_path_tag_mismatches(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = Computation::DetectPathTagMismatches;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to load config: {}", e),
            );
        }
    };

    // Check if any source dirs have schemas (direct or inherited).
    let has_any_schema = config.source_dirs.iter().any(|sd| sd.path_schema.is_some());
    if !has_any_schema {
        // No schemas configured — reconcile with empty set to clear stale signals.
        let (cleared, _, _, _) = reconcile_corpus_signals::<PathTagMismatchSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] DetectPathTagMismatches: cleared {} stale signals (no schemas configured)",
                cleared
            ));
        }
        return Result::success(computation, Vec::new());
    }

    // Load all corpus audio files with their tags.
    let files_with_tags = match read_only_db.get_all_audio_files_with_tags(Zone::Corpus, false) {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query corpus files: {}", e),
            );
        }
    };

    log_general(format!(
        "[COMPUTE] DetectPathTagMismatches: checking {} corpus files against path schemas",
        files_with_tags.len()
    ));

    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();
    let mut checked = 0;
    let mut structure_mismatches = 0;
    let mut value_mismatches = 0;

    for (audio_file, tag_map) in &files_with_tags {
        let corpus_path = audio_file.path();
        let relative_path = Path::new(corpus_path);

        // Resolve config for this path (includes schema via inheritance).
        let resolved = match config.resolve_source_config(relative_path) {
            Some(r) => r,
            None => continue,
        };

        // Find applicable schema.
        let schema = match resolved.path_schema.as_ref() {
            Some(s) => s,
            None => continue, // No schema for this path.
        };

        // Strip source dir prefix from the relative path.
        let sub_path = match relative_path.strip_prefix(&resolved.source_path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        // Strip file extension from the final component.
        let sub_path_no_ext = strip_extension(&sub_path);

        checked += 1;

        // Run the schema matcher.
        match schema.extract(&sub_path_no_ext) {
            PathSchemaMatchResult::Match(extracted) => {
                // Compare extracted path values against DB tags.
                let mismatches = compare_tags(&extracted, tag_map);
                if !mismatches.is_empty() {
                    value_mismatches += 1;
                    let signal = PathTagMismatchSignal {
                        inode: audio_file.inode(),
                        path: corpus_path.to_string(),
                        data: PathTagMismatchData {
                            source_dir: resolved.source_path.display().to_string(),
                            schema_template: schema.template.clone(),
                            mismatch_kind: PathMismatchKind::ValueMismatch { mismatches },
                        },
                    };
                    computed.push(ComputedCorpusSignal::new(
                        signal.inode,
                        TypedSignalWrite::PathTagMismatch(signal),
                    ));
                }
            }
            PathSchemaMatchResult::StructureMismatch(description) => {
                structure_mismatches += 1;
                let signal = PathTagMismatchSignal {
                    inode: audio_file.inode(),
                    path: corpus_path.to_string(),
                    data: PathTagMismatchData {
                        source_dir: resolved.source_path.display().to_string(),
                        schema_template: schema.template.clone(),
                        mismatch_kind: PathMismatchKind::StructureMismatch { description },
                    },
                };
                computed.push(ComputedCorpusSignal::new(
                    signal.inode,
                    TypedSignalWrite::PathTagMismatch(signal),
                ));
            }
        }
    }

    let (cleared, new, updated, unchanged) =
        reconcile_corpus_signals::<PathTagMismatchSignal>(read_only_db, &sender, computed, witness);

    log_general(format!(
        "[COMPUTE] DetectPathTagMismatches: checked={}, structure_mismatches={}, value_mismatches={} | \
         signals: cleared={}, new={}, updated={}, unchanged={}",
        checked, structure_mismatches, value_mismatches, cleared, new, updated, unchanged
    ));

    Result::success(computation, Vec::new())
}

/// Strip the file extension from a path string.
///
/// "Artist - Title.flac" → "Artist - Title"
/// "no-extension" → "no-extension"
fn strip_extension(path: &str) -> String {
    // Only strip extension from the last path component.
    if let Some(last_dot) = path.rfind('.') {
        // Make sure the dot is in the last segment (after the last `/`).
        let last_slash = path.rfind('/').unwrap_or(0);
        if last_dot > last_slash {
            return path[..last_dot].to_string();
        }
    }
    path.to_string()
}

/// Compare path-extracted tag values against DB tag values.
///
/// Returns a list of mismatches. Case-insensitive comparison.
/// Tags in DB are keyed by uppercase name; values are Vec<String>
/// (multi-valued tags). We check if the path value matches ANY of the DB values.
fn compare_tags(
    extracted: &std::collections::HashMap<String, String>,
    db_tags: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<PathTagValueMismatch> {
    let mut mismatches = Vec::new();

    for (tag_name, path_value) in extracted {
        let tag_upper = tag_name.to_uppercase();
        match db_tags.get(&tag_upper) {
            Some(db_values) => {
                // Check case-insensitive match against any DB value.
                let path_lower = path_value.to_lowercase();
                let matches = db_values.iter().any(|v| v.to_lowercase() == path_lower);
                if !matches {
                    mismatches.push(PathTagValueMismatch {
                        tag_name: tag_upper,
                        path_value: path_value.clone(),
                        db_value: Some(db_values.first().cloned().unwrap_or_default()),
                    });
                }
            }
            None => {
                // Tag not in DB at all.
                mismatches.push(PathTagValueMismatch {
                    tag_name: tag_upper,
                    path_value: path_value.clone(),
                    db_value: None,
                });
            }
        }
    }

    mismatches
}
