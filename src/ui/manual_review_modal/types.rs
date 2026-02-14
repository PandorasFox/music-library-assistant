//! Manual Review Modal Types
//!
//! Data structures for the manual review modal, including review groups,
//! file entries, and data loading from signal tables.

use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::file_ops::MoveToStashMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;

/// What kind of manual review this modal is performing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    /// Equal-quality duplicates (same fingerprint + quality score).
    /// Only stashing resolves these — tag edits cannot help.
    RedundantDuplicate,
    /// Multiple corpus files deploy to the same library path.
    /// Tag editing or stashing resolves these.
    DeployConflict,
    /// Files with identical tag signatures (artist/album/title).
    /// Tag editing or stashing resolves these.
    MetadataDuplicate,
}

impl ReviewKind {
    /// Human-readable title for the modal header.
    pub fn title(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "Redundant Duplicate Review",
            Self::DeployConflict => "Deploy Conflict Review",
            Self::MetadataDuplicate => "Metadata Duplicate Review",
        }
    }

    /// Stash directory name for this kind.
    pub fn stash_name(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "redundant",
            Self::DeployConflict => "deploy_conflict",
            Self::MetadataDuplicate => "metadata_dup",
        }
    }

    /// Transaction label.
    pub fn transaction_label(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "Redundant duplicate resolution",
            Self::DeployConflict => "Deploy conflict resolution",
            Self::MetadataDuplicate => "Metadata duplicate resolution",
        }
    }

    /// Whether tag editing is available for this kind.
    pub fn supports_tag_edit(self) -> bool {
        true
    }
}

/// Audio metadata summary for the detail pane (loaded once at modal init).
#[derive(Debug, Clone, Default)]
pub struct FileMetaSummary {
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub file_size: i64,
    pub has_pictures: bool,
    /// Ordered list of (tag_name, tag_value).
    pub tags: Vec<(String, String)>,
}

/// A single file entry within a review group.
#[derive(Debug, Clone)]
pub struct ReviewFileEntry {
    /// Corpus-relative path.
    pub corpus_path: String,
    /// Inode for DB operations.
    pub inode: i64,
    /// Kind-specific context line (deploy path, quality description, tag signature).
    pub context: String,
    /// Whether this file has been marked for stashing in this session.
    pub stashed: bool,
    /// Audio metadata (loaded at init time).
    pub meta: Option<FileMetaSummary>,
}

/// A group of files requiring review together.
#[derive(Debug, Clone)]
pub struct ReviewGroup {
    /// Human-readable label for this group (fingerprint, deploy path, tag signature).
    pub label: String,
    /// Files in this group.
    pub files: Vec<ReviewFileEntry>,
}

/// Cached data for the manual review modal.
#[derive(Debug, Clone, Default)]
pub struct ManualReviewData {
    /// All groups to review.
    pub groups: Vec<ReviewGroup>,
}

impl ManualReviewData {
    /// Load review data from the database based on review kind.
    pub fn load(read_db: &ReadOnlyDb<'_>, kind: ReviewKind) -> Result<Self> {
        let mut data = match kind {
            ReviewKind::RedundantDuplicate => Self::load_redundant_duplicates(read_db)?,
            ReviewKind::DeployConflict => Self::load_deploy_conflicts(read_db)?,
            ReviewKind::MetadataDuplicate => Self::load_metadata_duplicates(read_db)?,
        };
        data.enrich_with_metadata(read_db);
        Ok(data)
    }

    fn load_redundant_duplicates(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        let signal_groups = read_db.get_redundant_duplicate_groups()?;

        let mut groups = Vec::new();
        for (_key, data) in signal_groups {
            let label = format!("{} ({}×, quality {})", data.file_type, data.inodes.len(), data.quality_score);

            let mut files = Vec::new();
            for (idx, &inode) in data.inodes.iter().enumerate() {
                let path = data.paths.get(idx)
                    .cloned()
                    .unwrap_or_else(|| {
                        read_db.get_corpus_path_for_inode(inode)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| format!("<inode {}>", inode))
                    });

                files.push(ReviewFileEntry {
                    corpus_path: path,
                    inode,
                    context: format!("Quality score: {}/999", data.quality_score),
                    stashed: false,
                    meta: None,
                });
            }

            if files.len() >= 2 {
                groups.push(ReviewGroup { label, files });
            }
        }

        Ok(Self { groups })
    }

    fn load_deploy_conflicts(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        let conflict_groups = read_db.get_deploy_conflict_groups()?;

        let mut groups = Vec::new();
        for group in conflict_groups {
            let label = group.deploy_path.clone();

            let files: Vec<ReviewFileEntry> = group.conflicting_files
                .into_iter()
                .map(|(corpus_path, inode)| {
                    ReviewFileEntry {
                        corpus_path,
                        inode,
                        context: format!("Deploys to: {}", group.deploy_path),
                        stashed: false,
                        meta: None,
                    }
                })
                .collect();

            if files.len() >= 2 {
                groups.push(ReviewGroup { label, files });
            }
        }

        Ok(Self { groups })
    }

    fn load_metadata_duplicates(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        let signal_groups = read_db.get_metadata_duplicate_groups()?;

        let mut groups = Vec::new();
        for (_key, data) in signal_groups {
            let label = data.tag_signature.clone();

            let mut files = Vec::new();
            for &inode in &data.inodes {
                let path = read_db.get_corpus_path_for_inode(inode)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| format!("<inode {}>", inode));

                files.push(ReviewFileEntry {
                    corpus_path: path,
                    inode,
                    context: data.tag_signature.clone(),
                    stashed: false,
                    meta: None,
                });
            }

            if files.len() >= 2 {
                groups.push(ReviewGroup { label, files });
            }
        }

        Ok(Self { groups })
    }

    /// Enrich all file entries with audio metadata and tags from the database.
    fn enrich_with_metadata(&mut self, read_db: &ReadOnlyDb<'_>) {
        for group in &mut self.groups {
            for file in &mut group.files {
                let audio_info = read_db.get_audio_info(file.inode).ok().flatten();
                let tags = read_db.get_corpus_tags(file.inode).ok().unwrap_or_default();
                let has_pictures = read_db.get_has_pictures(file.inode).unwrap_or(false);

                if let Some(info) = audio_info {
                    // Get file_size from files table
                    let file_size = read_db
                        .get_audio_file_by_inode(file.inode, crate::corpus::db::types::FileSource::Corpus)
                        .ok()
                        .flatten()
                        .map(|af| af.entry.file_size)
                        .unwrap_or(0);

                    file.meta = Some(FileMetaSummary {
                        file_type: info.file_type,
                        duration_ms: info.duration_ms,
                        bitrate_kbps: info.bitrate_kbps,
                        sample_rate: info.sample_rate,
                        file_size,
                        has_pictures,
                        tags: tags.into_iter().map(|t| (t.tag_name, t.tag_value)).collect(),
                    });
                }
            }
        }
    }

    /// Whether there are any groups to review.
    pub fn has_groups(&self) -> bool {
        !self.groups.is_empty()
    }
}

/// Generate MoveToStash + DropFromIndex mutations for a single file.
pub fn stash_file_mutations(corpus_path: &str, inode: i64, stash_name: &str) -> Vec<Mutation> {
    let resolver = paths::get_resolver();
    let abs_path = resolver.resolve(std::path::Path::new(corpus_path));

    vec![
        Mutation::MoveToStash(MoveToStashMutation {
            path: abs_path,
            stash_name: stash_name.to_string(),
        }),
        Mutation::DropFromIndex(DropFromIndexMutation {
            path: PathBuf::from(corpus_path),
            inode: Some(inode),
            source: Some("corpus".to_string()),
        }),
    ]
}
