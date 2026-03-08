//! Runtime memory diagnostics.
//!
//! Reads `/proc/self/status` and `/proc/self/smaps_rollup` for process-level
//! memory stats, queries SQLite's global allocator, and logs a periodic
//! breakdown to `general.log`.

use std::time::{Duration, Instant};

/// How often to log a memory snapshot.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(30);

/// State for periodic memory logging.
pub struct MemoryDiagnostics {
    last_snapshot: Instant,
}

impl MemoryDiagnostics {
    pub fn new() -> Self {
        Self {
            last_snapshot: Instant::now(),
        }
    }

    /// Called each frame. Logs a snapshot if enough time has elapsed.
    pub fn tick(&mut self) {
        if !crate::config::is_memory_logging_enabled() {
            return;
        }
        if self.last_snapshot.elapsed() >= SNAPSHOT_INTERVAL {
            self.last_snapshot = Instant::now();
            log_memory_snapshot();
        }
    }

    /// Force an immediate snapshot (e.g., on startup, on view transition).
    pub fn snapshot_now(&mut self) {
        if !crate::config::is_memory_logging_enabled() {
            return;
        }
        self.last_snapshot = Instant::now();
        log_memory_snapshot();
    }
}

// ============================================================================
// Snapshot
// ============================================================================

fn log_memory_snapshot() {
    let mut parts = Vec::new();

    // 1. /proc/self/status — VmRSS, VmSize, VmData, RssAnon, RssFile, RssShmem
    if let Some(proc_stats) = read_proc_status() {
        parts.push(format!(
            "RSS={:.1}MB (anon={:.1}MB file={:.1}MB shmem={:.1}MB) VmSize={:.1}MB VmData={:.1}MB",
            proc_stats.vm_rss_mb,
            proc_stats.rss_anon_mb,
            proc_stats.rss_file_mb,
            proc_stats.rss_shmem_mb,
            proc_stats.vm_size_mb,
            proc_stats.vm_data_mb,
        ));
    }

    // 2. SQLite global memory (all connections combined)
    let sqlite_used = unsafe { rusqlite::ffi::sqlite3_memory_used() };
    let sqlite_highwater = unsafe { rusqlite::ffi::sqlite3_memory_highwater(0) };
    parts.push(format!(
        "SQLite={:.1}MB (highwater={:.1}MB)",
        sqlite_used as f64 / (1024.0 * 1024.0),
        sqlite_highwater as f64 / (1024.0 * 1024.0),
    ));

    // 3. /proc/self/smaps_rollup — PSS, Swap
    if let Some(smaps) = read_smaps_rollup() {
        parts.push(format!(
            "PSS={:.1}MB Swap={:.1}MB",
            smaps.pss_mb, smaps.swap_mb,
        ));
    }

    // 4. Thread count from /proc/self/status
    if let Some(threads) = read_thread_count() {
        parts.push(format!("threads={}", threads));
    }

    crate::logging::log_general(format!("[MEMORY] {}", parts.join(" | ")));
}

// ============================================================================
// /proc/self/status parsing
// ============================================================================

struct ProcStatus {
    vm_rss_mb: f64,
    vm_size_mb: f64,
    vm_data_mb: f64,
    rss_anon_mb: f64,
    rss_file_mb: f64,
    rss_shmem_mb: f64,
}

fn read_proc_status() -> Option<ProcStatus> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut stats = ProcStatus {
        vm_rss_mb: 0.0,
        vm_size_mb: 0.0,
        vm_data_mb: 0.0,
        rss_anon_mb: 0.0,
        rss_file_mb: 0.0,
        rss_shmem_mb: 0.0,
    };
    for line in content.lines() {
        if let Some(kb) = parse_kb_line(line, "VmRSS:") {
            stats.vm_rss_mb = kb / 1024.0;
        } else if let Some(kb) = parse_kb_line(line, "VmSize:") {
            stats.vm_size_mb = kb / 1024.0;
        } else if let Some(kb) = parse_kb_line(line, "VmData:") {
            stats.vm_data_mb = kb / 1024.0;
        } else if let Some(kb) = parse_kb_line(line, "RssAnon:") {
            stats.rss_anon_mb = kb / 1024.0;
        } else if let Some(kb) = parse_kb_line(line, "RssFile:") {
            stats.rss_file_mb = kb / 1024.0;
        } else if let Some(kb) = parse_kb_line(line, "RssShmem:") {
            stats.rss_shmem_mb = kb / 1024.0;
        }
    }
    Some(stats)
}

fn read_thread_count() -> Option<u32> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Threads:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Parse a line like "VmRSS:    1234 kB" → Some(1234.0)
fn parse_kb_line(line: &str, prefix: &str) -> Option<f64> {
    let rest = line.strip_prefix(prefix)?;
    let rest = rest.trim();
    let rest = rest
        .strip_suffix("kB")
        .or_else(|| rest.strip_suffix("KB"))?;
    rest.trim().parse::<f64>().ok()
}

// ============================================================================
// /proc/self/smaps_rollup parsing
// ============================================================================

struct SmapsRollup {
    pss_mb: f64,
    swap_mb: f64,
}

fn read_smaps_rollup() -> Option<SmapsRollup> {
    let content = std::fs::read_to_string("/proc/self/smaps_rollup").ok()?;
    let mut pss_kb = 0.0_f64;
    let mut swap_kb = 0.0_f64;
    for line in content.lines() {
        // Use exact "Pss:" to avoid matching "Pss_Anon:" etc.
        if line.starts_with("Pss:") && !line.starts_with("Pss_") {
            if let Some(kb) = parse_kb_line(line, "Pss:") {
                pss_kb = kb;
            }
        } else if line.starts_with("Swap:") && !line.starts_with("SwapPss:") {
            if let Some(kb) = parse_kb_line(line, "Swap:") {
                swap_kb = kb;
            }
        }
    }
    Some(SmapsRollup {
        pss_mb: pss_kb / 1024.0,
        swap_mb: swap_kb / 1024.0,
    })
}
