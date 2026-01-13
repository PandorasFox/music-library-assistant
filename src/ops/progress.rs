use std::time::{Duration, Instant};

/// Statistics about mtime mismatches during incremental scan detection
#[derive(Debug, Clone, Default)]
pub struct MtimeMismatchStats {
    /// Number of files with inode found in DB but mtime didn't match
    pub mismatch_count: usize,
    /// Number of files with inode not found in DB at all
    pub not_in_db_count: usize,
    /// Differences in seconds (file_mtime - db_mtime) for mismatched files
    pub diffs_secs: Vec<i64>,
}

impl MtimeMismatchStats {
    pub fn mean_diff_secs(&self) -> Option<f64> {
        if self.diffs_secs.is_empty() {
            return None;
        }
        let sum: i64 = self.diffs_secs.iter().sum();
        Some(sum as f64 / self.diffs_secs.len() as f64)
    }

    pub fn median_diff_secs(&self) -> Option<i64> {
        if self.diffs_secs.is_empty() {
            return None;
        }
        let mut sorted = self.diffs_secs.clone();
        sorted.sort();
        let mid = sorted.len() / 2;
        if sorted.len().is_multiple_of(2) {
            Some((sorted[mid - 1] + sorted[mid]) / 2)
        } else {
            Some(sorted[mid])
        }
    }

    pub fn stddev_diff_secs(&self) -> Option<f64> {
        let mean = self.mean_diff_secs()?;
        if self.diffs_secs.len() < 2 {
            return None;
        }
        let variance: f64 = self.diffs_secs.iter()
            .map(|&x| {
                let diff = x as f64 - mean;
                diff * diff
            })
            .sum::<f64>() / (self.diffs_secs.len() - 1) as f64;
        Some(variance.sqrt())
    }

    pub fn mode_diff_secs(&self) -> Option<i64> {
        if self.diffs_secs.is_empty() {
            return None;
        }
        use std::collections::HashMap;
        let mut counts: HashMap<i64, usize> = HashMap::new();
        for &diff in &self.diffs_secs {
            *counts.entry(diff).or_insert(0) += 1;
        }
        counts.into_iter()
            .max_by_key(|&(_, count)| count)
            .map(|(val, _)| val)
    }
}

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub total_bytes: u64,
    pub bytes_processed: u64,
    pub files_processed: usize,
    pub total_files: usize,
    pub files_skipped: usize,  // Files skipped due to unchanged inode+mtime
    pub current_file: Option<String>,
    pub errors: usize,
    pub start_time: Instant,
    /// Statistics about why files weren't skipped (mtime mismatches)
    pub mtime_stats: Option<MtimeMismatchStats>,
}

impl ScanProgress {
    pub fn percentage(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        let pct = (self.bytes_processed as f64 / self.total_bytes as f64) * 100.0;
        pct.clamp(0.0, 100.0) as u8
    }

    pub fn throughput_mbps(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 {
            return 0.0;
        }
        let mb_processed = self.bytes_processed as f64 / 1_000_000.0;
        mb_processed / elapsed
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.bytes_processed == 0 {
            return None;
        }

        let throughput = self.throughput_mbps();
        if throughput == 0.0 {
            return None;
        }

        let remaining_bytes = self.total_bytes.saturating_sub(self.bytes_processed);
        let remaining_mb = remaining_bytes as f64 / 1_000_000.0;
        Some((remaining_mb / throughput) as u64)
    }
}

#[derive(Debug, Clone)]
pub enum ScanMessage {
    Progress(ScanProgress),
    Complete(ScanResult),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub files_scanned: usize,
    pub files_skipped: usize,
    pub bytes_scanned: u64,
    pub errors: usize,
    pub duration: Duration,
}
