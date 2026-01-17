//! Shared UI rendering utilities
//!
//! Common helpers used across multiple UI modules to avoid code duplication.

use crate::config;
use crate::corpus::db::Database;

// Re-export centered_rect from widgets module
pub use super::widgets::centered_rect;

// ============================================================================
// Formatting Utilities
// ============================================================================

/// Truncate a string from the left, UTF-8 safe. Result: `...visible_end`
///
/// Keeps the rightmost `max_chars` characters. Useful for paths where
/// the filename/end is most relevant.
pub fn truncate_left(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    let skip = char_count.saturating_sub(max_chars - 3);
    format!("...{}", s.chars().skip(skip).collect::<String>())
}

/// Truncate a string from the right, UTF-8 safe. Result: `visible_start...`
///
/// Keeps the leftmost `max_chars` characters. Useful for tags/labels where
/// the start is most relevant.
pub fn truncate_right(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    format!("{}...", s.chars().take(max_chars - 3).collect::<String>())
}

/// Alias for backward compatibility.
#[inline]
pub fn truncate_path_display(path: &str, max_len: usize) -> String {
    truncate_left(path, max_len)
}

/// Format bytes using binary SI units (KiB, MiB, GiB, TiB).
pub fn format_bytes_binary(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= TIB {
        format!("{:.2} TiB", bytes_f / TIB)
    } else if bytes_f >= GIB {
        format!("{:.2} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.0} KiB", bytes_f / KIB)
    } else {
        format!("{} B", bytes)
    }
}

/// Format duration from milliseconds as m:ss or h:mm:ss.
///
/// Returns "Unknown" if None is passed.
pub fn format_duration_ms(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => {
            let total_secs = ms / 1000;
            if total_secs >= 3600 {
                let hours = total_secs / 3600;
                let mins = (total_secs % 3600) / 60;
                let secs = total_secs % 60;
                format!("{}:{:02}:{:02}", hours, mins, secs)
            } else {
                let mins = total_secs / 60;
                let secs = total_secs % 60;
                format!("{}:{:02}", mins, secs)
            }
        }
        None => "Unknown".to_string(),
    }
}

/// Format ETA as mm:ss or h:mm:ss.
pub fn format_eta(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        let secs = seconds % 60;
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        let mins = seconds / 60;
        let secs = seconds % 60;
        format!("{}:{:02}", mins, secs)
    }
}

/// Format a Duration as a human-readable string.
pub fn format_duration(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if total_secs == 0 {
        format!("{}ms", millis)
    } else if total_secs < 60 {
        format!("{}.{}s", total_secs, millis / 100)
    } else if total_secs < 3600 {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{}m {}s", mins, secs)
    } else {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        format!("{}h {}m", hours, mins)
    }
}

/// Calculate rolling average throughput in MiB/s from recent samples.
///
/// Uses samples from the last `window_secs` seconds.
pub fn calculate_rolling_throughput(
    samples: &std::collections::VecDeque<(std::time::Instant, u64)>,
    window_secs: u64,
) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }

    let now = std::time::Instant::now();
    let window_start = now - std::time::Duration::from_secs(window_secs);

    // Find samples within the window
    let samples_in_window: Vec<_> = samples
        .iter()
        .filter(|(t, _)| *t >= window_start)
        .collect();

    if samples_in_window.len() < 2 {
        return None;
    }

    let first = samples_in_window.first()?;
    let last = samples_in_window.last()?;

    let time_diff = last.0.duration_since(first.0).as_secs_f64();
    if time_diff < 0.1 {
        return None;
    }

    let bytes_diff = last.1.saturating_sub(first.1);
    let mib_per_sec = (bytes_diff as f64 / (1024.0 * 1024.0)) / time_diff;

    Some(mib_per_sec)
}

// ============================================================================
// Database Helpers
// ============================================================================

/// Helper to initialize database connection with standard error handling.
///
/// Returns `None` and sets status message on the app if an error occurs.
/// This eliminates the repeated db initialization boilerplate throughout flow handlers.
pub fn open_database(status_message: &mut Option<String>) -> Option<Database> {
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(e) => {
            *status_message = Some(format!("Config error: {}", e));
            return None;
        }
    };
    match Database::open(&db_path) {
        Ok(db) => Some(db),
        Err(e) => {
            *status_message = Some(format!("Database error: {}", e));
            None
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_path_display_short() {
        let path = "/home/user/file.mp3";
        assert_eq!(truncate_path_display(path, 50), path);
    }

    #[test]
    fn test_truncate_path_display_exact() {
        let path = "exactly20chars!!!"; // 17 chars
        assert_eq!(truncate_path_display(path, 17), path);
    }

    #[test]
    fn test_truncate_path_display_long() {
        let path = "/very/long/path/to/some/deeply/nested/file.mp3";
        let truncated = truncate_path_display(path, 20);
        assert!(truncated.starts_with("..."));
        assert_eq!(truncated.chars().count(), 20);
    }

    #[test]
    fn test_truncate_path_display_unicode() {
        // Unicode characters should be handled correctly
        let path = "/home/用户/音乐/歌曲.mp3";
        let truncated = truncate_path_display(path, 15);
        assert!(truncated.starts_with("..."));
        assert_eq!(truncated.chars().count(), 15);
    }

    #[test]
    fn test_format_bytes_binary_bytes() {
        assert_eq!(format_bytes_binary(0), "0 B");
        assert_eq!(format_bytes_binary(512), "512 B");
        assert_eq!(format_bytes_binary(1023), "1023 B");
    }

    #[test]
    fn test_format_bytes_binary_kib() {
        assert_eq!(format_bytes_binary(1024), "1 KiB");
        assert_eq!(format_bytes_binary(1536), "2 KiB"); // 1.5 rounds to 2
        assert_eq!(format_bytes_binary(512 * 1024), "512 KiB");
    }

    #[test]
    fn test_format_bytes_binary_mib() {
        assert_eq!(format_bytes_binary(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes_binary(1024 * 1024 * 100), "100.0 MiB");
        assert_eq!(format_bytes_binary(1024 * 1024 * 500), "500.0 MiB");
    }

    #[test]
    fn test_format_bytes_binary_gib() {
        assert_eq!(format_bytes_binary(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(format_bytes_binary(1024 * 1024 * 1024 * 50), "50.00 GiB");
    }

    #[test]
    fn test_format_bytes_binary_tib() {
        assert_eq!(format_bytes_binary(1024_u64 * 1024 * 1024 * 1024), "1.00 TiB");
        assert_eq!(format_bytes_binary(1024_u64 * 1024 * 1024 * 1024 * 2), "2.00 TiB");
    }

    #[test]
    fn test_format_eta_seconds() {
        assert_eq!(format_eta(0), "0:00");
        assert_eq!(format_eta(30), "0:30");
        assert_eq!(format_eta(59), "0:59");
    }

    #[test]
    fn test_format_eta_minutes() {
        assert_eq!(format_eta(60), "1:00");
        assert_eq!(format_eta(90), "1:30");
        assert_eq!(format_eta(3599), "59:59");
    }

    #[test]
    fn test_format_eta_hours() {
        assert_eq!(format_eta(3600), "1:00:00");
        assert_eq!(format_eta(3661), "1:01:01");
        assert_eq!(format_eta(7200), "2:00:00");
        assert_eq!(format_eta(86399), "23:59:59");
    }

    #[test]
    fn test_calculate_rolling_throughput_insufficient_samples() {
        use std::collections::VecDeque;
        use std::time::Instant;

        // Empty samples
        let samples: VecDeque<(Instant, u64)> = VecDeque::new();
        assert!(calculate_rolling_throughput(&samples, 10).is_none());

        // Single sample
        let mut samples = VecDeque::new();
        samples.push_back((Instant::now(), 1000));
        assert!(calculate_rolling_throughput(&samples, 10).is_none());
    }
}
