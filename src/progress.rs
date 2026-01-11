use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub total_bytes: u64,
    pub bytes_processed: u64,
    pub files_processed: usize,
    pub current_file: Option<String>,
    pub errors: usize,
    pub start_time: Instant,
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
