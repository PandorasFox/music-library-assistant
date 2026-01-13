//! Work unit grouping for efficient batch execution.
//!
//! Groups mutations by file path so that operations on the same file
//! can be batched together (e.g., multiple tag edits in one read-modify-write cycle).

use std::collections::HashMap;
use std::path::PathBuf;

use super::types::{Mutation, MutationCategory, WorkUnit};

/// Group mutations by file path for efficient execution.
///
/// This function:
/// 1. Groups file-based mutations by their primary path
/// 2. Separates DB-only mutations (no file path) into individual work units
/// 3. Preserves mutation order within each group
///
/// # Returns
///
/// A vector of `WorkUnit`s, each containing all mutations for a single file
/// or a single DB-only mutation.
pub fn group_by_file(mutations: Vec<Mutation>) -> Vec<WorkUnit> {
    let mut groups: HashMap<PathBuf, Vec<Mutation>> = HashMap::new();
    let mut db_only: Vec<Mutation> = Vec::new();

    for mutation in mutations {
        if let Some(path) = mutation.primary_path() {
            groups
                .entry(path.to_path_buf())
                .or_default()
                .push(mutation);
        } else {
            // DB-only mutations get their own work unit
            db_only.push(mutation);
        }
    }

    let mut units: Vec<WorkUnit> = groups
        .into_iter()
        .map(|(path, mutations)| {
            // Try to extract track_id from mutations if available
            let track_id = mutations.iter().find_map(|m| match m {
                Mutation::TagEditAndFlush { track_id, .. } => Some(*track_id),
                Mutation::IndexTrack { .. } => None, // Will be assigned during execution
                Mutation::Move { track_id, .. } => *track_id,
                Mutation::Delete { track_id, .. } => *track_id,
                Mutation::MoveToStash { track_id, .. } => *track_id,
                _ => None,
            });

            WorkUnit {
                path,
                track_id,
                mutations,
            }
        })
        .collect();

    // Add DB-only mutations as separate work units
    for mutation in db_only {
        let track_id = match &mutation {
            Mutation::TagEditDb { track_id, .. } => Some(*track_id),
            _ => None,
        };

        units.push(WorkUnit {
            path: PathBuf::new(),
            track_id,
            mutations: vec![mutation],
        });
    }

    units
}

/// Group mutations within a work unit by category for efficient execution.
///
/// This is used by the executor to batch same-type operations together.
/// For example, all tag edits for a file are executed in a single
/// read-modify-write cycle.
pub fn group_by_category(mutations: Vec<Mutation>) -> HashMap<MutationCategory, Vec<Mutation>> {
    let mut groups: HashMap<MutationCategory, Vec<Mutation>> = HashMap::new();

    for mutation in mutations {
        groups
            .entry(mutation.category())
            .or_default()
            .push(mutation);
    }

    groups
}

/// Sort work units to prioritize migrations first.
///
/// Migration mutations must be executed before other operations
/// to ensure the database schema is up to date.
pub fn sort_work_units(mut units: Vec<WorkUnit>) -> Vec<WorkUnit> {
    units.sort_by_key(|unit| {
        // Migrations first (0), then everything else (1)
        if unit.mutations.iter().any(|m| m.requires_serial()) {
            0
        } else {
            1
        }
    });
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_by_file() {
        let mutations = vec![
            Mutation::TagFlushToDisk {
                path: PathBuf::from("/test/a.flac"),
                tags: vec![("artist".to_string(), "Artist1".to_string())],
            },
            Mutation::TagFlushToDisk {
                path: PathBuf::from("/test/a.flac"),
                tags: vec![("album".to_string(), "Album1".to_string())],
            },
            Mutation::TagFlushToDisk {
                path: PathBuf::from("/test/b.flac"),
                tags: vec![("artist".to_string(), "Artist2".to_string())],
            },
            Mutation::TagEditDb {
                track_id: 1,
                tag_name: "genre".to_string(),
                old_value: None,
                new_value: Some("Rock".to_string()),
            },
        ];

        let units = group_by_file(mutations);

        // Should have 3 work units: 2 files + 1 DB-only
        assert_eq!(units.len(), 3);

        // Find the unit for /test/a.flac
        let a_unit = units
            .iter()
            .find(|u| u.path == PathBuf::from("/test/a.flac"))
            .expect("Should have unit for a.flac");

        // a.flac should have 2 mutations (both tag flushes)
        assert_eq!(a_unit.mutations.len(), 2);

        // b.flac should have 1 mutation
        let b_unit = units
            .iter()
            .find(|u| u.path == PathBuf::from("/test/b.flac"))
            .expect("Should have unit for b.flac");
        assert_eq!(b_unit.mutations.len(), 1);

        // DB-only should have 1 mutation
        let db_unit = units
            .iter()
            .find(|u| u.path == PathBuf::new())
            .expect("Should have DB-only unit");
        assert_eq!(db_unit.mutations.len(), 1);
        assert_eq!(db_unit.track_id, Some(1));
    }

    #[test]
    fn test_group_by_category() {
        let mutations = vec![
            Mutation::TagEditDb {
                track_id: 1,
                tag_name: "artist".to_string(),
                old_value: None,
                new_value: Some("Artist".to_string()),
            },
            Mutation::TagFlushToDisk {
                path: PathBuf::from("/test/a.flac"),
                tags: vec![],
            },
            Mutation::Move {
                source: PathBuf::from("/a"),
                destination: PathBuf::from("/b"),
                track_id: Some(1),
            },
        ];

        let groups = group_by_category(mutations);

        assert_eq!(groups.get(&MutationCategory::TagEdit).map(|v| v.len()), Some(2));
        assert_eq!(groups.get(&MutationCategory::FileMove).map(|v| v.len()), Some(1));
    }

    #[test]
    fn test_sort_work_units() {
        let units = vec![
            WorkUnit {
                path: PathBuf::from("/test/a.flac"),
                track_id: None,
                mutations: vec![Mutation::TagFlushToDisk {
                    path: PathBuf::from("/test/a.flac"),
                    tags: vec![],
                }],
            },
            WorkUnit {
                path: PathBuf::new(),
                track_id: None,
                mutations: vec![Mutation::DbMigration {
                    migration_id: 3,
                    description: "test".to_string(),
                }],
            },
        ];

        let sorted = sort_work_units(units);

        // Migration should come first
        assert!(sorted[0].mutations[0].requires_serial());
        assert!(!sorted[1].mutations[0].requires_serial());
    }
}
