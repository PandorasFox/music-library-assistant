//! Insights view data aggregation queries.

use anyhow::Result;
use rusqlite::params;

use super::super::Database;

impl Database {
    /// Get InsightsData for the Insights view.
    ///
    /// Computes all bucket data via SQL queries. Called by UiReadCache.
    pub fn get_insights_data(&self) -> Result<crate::meta::views::InsightsData> {
        use crate::meta::views::*;

        Ok(InsightsData {
            bucket_corpus: self.compute_corpus_files_bucket()?,
            bucket_placeholder: self.compute_tag_resolution_bucket()?,
            bucket_other: self.compute_other_signals_bucket()?,
        })
    }

    pub(super) fn compute_corpus_files_bucket(&self) -> Result<crate::meta::views::CorpusFilesBucket> {
        use crate::meta::views::*;

        // OOB signals (highest priority)
        let oob_tag_sync = self.count_signal_type("oob_tag_sync")?;
        let oob_tag_conflict = self.count_signal_type("oob_tag_conflict")?;
        let mtime_only_mismatch = self.count_signal_type("mtime_only_mismatch")?;

        // Standard corpus file signals
        let files_in_corpus = self.count_signal_type("file_in_corpus")?;
        let files_indexed = self.get_audio_file_count(Some("corpus")).unwrap_or(0);
        let files_unindexed = self.count_signal_type("unindexed_file")?;
        let files_missing = self.count_signal_type("missing_file")?;
        let directories_missing = self.count_signal_type("missing_directory")?;
        let files_relocated = self.count_signal_type("moved_file")?;

        // Error/format signals
        let corrupt_files = self.count_signal_type("corrupt_file")?;
        let shit_format_files = self.count_signal_type("shit_format")?;

        // Image files
        let images_in_corpus = self.get_image_file_count(Some("corpus")).unwrap_or(0);

        // File type breakdown
        let file_type_breakdown = self.get_file_type_breakdown()?;

        // Directory breakdown for FileInCorpus signals
        let directory_breakdown = self.get_directory_breakdown("file_in_corpus")?;

        Ok(CorpusFilesBucket {
            oob_tag_sync,
            oob_tag_conflict,
            mtime_only_mismatch,
            files_in_corpus,
            files_indexed,
            files_unindexed,
            files_missing,
            directories_missing,
            files_relocated,
            corrupt_files,
            shit_format_files,
            images_in_corpus,
            file_type_breakdown,
            _directory_breakdown: directory_breakdown,
        })
    }

    pub(super) fn compute_tag_resolution_bucket(&self) -> Result<crate::meta::views::TagSquashBucket> {
        use crate::meta::views::*;

        // Cross-source overlap clusters (easy resolutions - at top of bucket)
        // These are derived from fingerprint overlaps, clustered by source directory
        let cross_source_overlap_count = self.count_signal_type("cross_source_overlap")?;

        // Release overlaps (multiple releases → same album directory)
        let release_overlap_count = self.count_signal_type("release_overlap")?;

        // Subpar duplicates (lower quality versions identified by fingerprint analysis)
        let subpar_duplicate_count = self.count_signal_type("subpar_duplicate")?;

        // Redundant duplicates (equal quality, requires operator choice)
        let redundant_duplicate_count = self.count_signal_type("redundant_duplicate")?;

        // Count inconsistent_album_artist signals
        let inconsistent_album_artist_count =
            self.count_signal_type("inconsistent_album_artist")?;

        // Group compound_tag signals by tag name with safety classification
        let compound_tags = self.count_compound_signals_by_tag()?;

        // Group tag_canonicity signals by tag_name column
        // Sum inodes from bincode BLOB data
        let mut tag_map: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT tag_name, data FROM signal_tag_canonicity")?;
            let rows = stmt.query_map(params![], |row| {
                let tag_name: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((tag_name, blob))
            })?;
            for (tag_name, blob) in rows.flatten() {
                let inode_count =
                    bincode::deserialize::<crate::meta::signals::data::TagCanonicityData>(&blob)
                        .map(|d| d.inodes.len())
                        .unwrap_or(0);
                let entry = tag_map.entry(tag_name).or_insert((0, 0));
                entry.0 += 1; // cluster_count
                entry.1 += inode_count; // total_tracks
            }
        }
        // Drop the unused stmt (was a false start)
        let mut tag_canonicity: Vec<TagSquashEntry> = tag_map
            .into_iter()
            .map(|(tag_name, (cluster_count, total_tracks))| TagSquashEntry {
                tag_name,
                cluster_count,
                _total_tracks: total_tracks,
            })
            .collect();
        tag_canonicity.sort_by(|a, b| b._total_tracks.cmp(&a._total_tracks));

        let missing_album_single_count = self.count_signal_type("missing_album_single")?;

        let disc_extraction_count = self.count_signal_type("disc_extraction")?;

        let path_tag_mismatch_count = self.count_signal_type("path_tag_mismatch")?;

        let same_recording_different_release_count = self.count_signal_type("same_recording_different_release")?;

        Ok(TagSquashBucket {
            cross_source_overlap_count,
            release_overlap_count,
            subpar_duplicate_count,
            redundant_duplicate_count,
            tag_canonicity,
            inconsistent_album_artist_count,
            compound_tags,
            missing_album_single_count,
            disc_extraction_count,
            path_tag_mismatch_count,
            same_recording_different_release_count,
        })
    }

    /// Count unique compound tag values by safety classification, grouped by tag name.
    ///
    /// Counts unique (tag_name, compound_value) pairs rather than individual signals,
    /// so the insights view shows how many distinct compound values need resolution.
    /// Returns entries grouped by tag name, sorted by total count descending.
    pub(super) fn count_compound_signals_by_tag(&self) -> Result<Vec<crate::meta::views::CompoundTagEntry>> {
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;
        use crate::meta::views::CompoundTagEntry;
        use std::collections::{HashMap, HashSet};

        let mut stmt = self.conn.prepare("SELECT data FROM signal_compound_tag")?;

        // Track unique (tag_name, compound_value) per safety bucket
        let mut safe_seen: HashSet<(String, String)> = HashSet::new();
        let mut review_seen: HashSet<(String, String)> = HashSet::new();

        let rows = stmt.query_map(params![], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;

        for row in rows {
            let blob = match row {
                Ok(b) => b,
                Err(_) => continue,
            };

            let compounds: Vec<TypedEntry> = match bincode::deserialize(&blob) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for compound in &compounds {
                let key = (compound.tag_name.clone(), compound.compound_value.clone());
                if compound.is_safe() {
                    safe_seen.insert(key);
                } else {
                    review_seen.insert(key);
                }
            }
        }

        // Aggregate counts per tag_name
        let mut by_tag: HashMap<String, (usize, usize)> = HashMap::new();
        for (tag_name, _) in &safe_seen {
            by_tag.entry(tag_name.clone()).or_insert((0, 0)).0 += 1;
        }
        for (tag_name, _) in &review_seen {
            by_tag.entry(tag_name.clone()).or_insert((0, 0)).1 += 1;
        }

        let mut entries: Vec<CompoundTagEntry> = by_tag
            .into_iter()
            .map(|(tag_name, (safe_count, review_count))| CompoundTagEntry {
                tag_name,
                safe_count,
                review_count,
            })
            .collect();

        entries.sort_by(|a, b| {
            let total_a = a.safe_count + a.review_count;
            let total_b = b.safe_count + b.review_count;
            total_b.cmp(&total_a)
        });

        Ok(entries)
    }

    pub(super) fn compute_other_signals_bucket(&self) -> Result<crate::meta::views::OtherSignalsBucket> {
        use crate::meta::views::*;

        let mut entries = Vec::new();

        // Aggregate signals with affected counts
        // Note: fingerprint_overlap and subpar_duplicate are now in the TagSquash bucket
        for (signal_type, label) in [
            ("metadata_duplicate", "Metadata Duplicates"),
            ("duplicate_inode", "Duplicate Inodes"),
            ("missing_tag", "Missing Tags"),
            ("deploy_conflict", "Deploy Conflicts"),
        ] {
            let count = self.count_signal_type(signal_type)?;

            if count > 0 {
                let affected = self.count_affected_by_signal(signal_type)?;
                entries.push(OtherSignalEntry {
                    signal_type: signal_type.to_string(),
                    display_label: label.to_string(),
                    count,
                    affected_count: if affected > 0 { Some(affected) } else { None },
                });
            }
        }

        // Sort by count descending
        entries.sort_by(|a, b| b.count.cmp(&a.count));

        Ok(OtherSignalsBucket { entries })
    }

    /// Count signals of a specific type using typed tables.
    pub(super) fn count_signal_type(&self, signal_type: &str) -> Result<usize> {
        Ok(crate::meta::signals::registry::count_signal_type(&self.conn, signal_type)?)
    }

    /// Count tracks affected by aggregate signals.
    ///
    /// Reads from typed tables and counts inodes in bincode BLOB data.
    pub(super) fn count_affected_by_signal(&self, signal_type: &str) -> Result<usize> {
        let table = match signal_type {
            "metadata_duplicate" => "signal_metadata_duplicate",
            "duplicate_inode" => "signal_duplicate_inode",
            "missing_tag" => "signal_missing_tag",
            "deploy_conflict" => "signal_deploy_conflict",
            "sidecar_deploy_conflict" => "signal_sidecar_deploy_conflict",
            _ => return Ok(0),
        };
        // These tables store inodes in a bincode BLOB 'data' column.
        // Count rows and read blob to sum inode counts.
        let sql = format!("SELECT data FROM {}", table);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;

        let mut total = 0usize;
        for blob in rows.flatten() {
            // All these types have an inodes: Vec<i64> field in their data
            if let Ok(inodes) = bincode::deserialize::<Vec<i64>>(&blob) {
                total += inodes.len();
            }
        }
        Ok(total)
    }

    /// Get file type breakdown from audio files (files + audio_info).
    pub(super) fn get_file_type_breakdown(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT a.file_type, COUNT(*) as cnt
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.zone = 'corpus' AND f.is_dir = 0
               GROUP BY a.file_type
               ORDER BY cnt DESC"#,
        )?;

        let results = stmt
            .query_map(params![], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get directory breakdown for a signal type.
    pub(super) fn get_directory_breakdown(
        &self,
        signal_type: &str,
    ) -> Result<crate::meta::views::DirectoryBreakdown> {
        use crate::meta::views::*;

        // Map signal type to its typed table name
        let table = match signal_type {
            "file_in_corpus" => "signal_file_in_corpus",
            "unindexed_file" => "signal_unindexed_file",
            "healthy_file" => "signal_healthy_file",
            "missing_file" => "signal_missing_file",
            _ => {
                return Ok(DirectoryBreakdown {
                    _entries: Vec::new(),
                })
            }
        };

        // Extract parent directory from path column
        let sql = format!(
            r#"SELECT
                 CASE
                   WHEN instr(path, '/') > 0
                   THEN substr(path, 1, length(path) - length(replace(path, '/', '')) -
                        length(substr(path, length(path) - length(replace(path, '/', '')) + 1)))
                   ELSE ''
                 END as dir,
                 COUNT(*) as cnt
               FROM {}
               GROUP BY dir
               ORDER BY cnt DESC
               LIMIT 50"#,
            table
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let entries = stmt
            .query_map(params![], |row| {
                Ok(DirectoryBreakdownEntry {
                    _directory: row.get(0)?,
                    _count: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(DirectoryBreakdown { _entries: entries })
    }

    /// Run a GROUP BY query returning `(String, usize)` category counts.
    ///
    /// SQL must select exactly two columns: a text category and an integer count.
    pub(super) fn count_by_category(&self, sql: &str) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?)))?
            .flatten()
            .collect::<Vec<_>>();
        Ok(rows)
    }
}
