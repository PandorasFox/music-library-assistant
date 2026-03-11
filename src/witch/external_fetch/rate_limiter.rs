//! Rate limiting for external API calls.

use std::time::{Duration, Instant};

/// Per-source rate limiter with exponential backoff support.
pub(super) struct RateLimiter {
    /// Minimum interval between requests.
    base_interval: Duration,
    /// Current backoff multiplier (1 = no backoff).
    backoff_multiplier: u32,
    /// When the last request was issued.
    last_request_at: Option<Instant>,
    /// Maximum backoff multiplier.
    max_backoff: u32,
}

impl RateLimiter {
    pub(super) fn new_acoustid(requests_per_second: u32) -> Self {
        Self {
            base_interval: Duration::from_millis(1000 / requests_per_second.max(1) as u64),
            backoff_multiplier: 1,
            last_request_at: None,
            max_backoff: 1, // AcoustID uses fixed 2s penalty, not exponential
        }
    }

    /// Duration until this limiter is ready for another request.
    /// Returns `Duration::ZERO` if ready now.
    pub(super) fn time_until_ready(&self) -> Duration {
        match self.last_request_at {
            None => Duration::ZERO,
            Some(last) => {
                let effective = self.base_interval * self.backoff_multiplier;
                effective.saturating_sub(last.elapsed())
            }
        }
    }

    /// Mark that a request was just dispatched.
    pub(super) fn mark_request(&mut self) {
        self.last_request_at = Some(Instant::now());
    }

    /// Double the backoff multiplier (capped at max_backoff).
    pub(super) fn apply_backoff(&mut self) {
        self.backoff_multiplier = (self.backoff_multiplier * 2).min(self.max_backoff);
    }

    /// Reset backoff to normal rate.
    pub(super) fn reset_backoff(&mut self) {
        self.backoff_multiplier = 1;
    }

    /// Current effective requests per second (accounting for backoff).
    pub(super) fn effective_rps(&self) -> f64 {
        let effective_micros =
            self.base_interval.as_micros() as f64 * self.backoff_multiplier.max(1) as f64;
        1_000_000.0 / effective_micros
    }
}

/// Initial MB requests per second (conservative start).
pub(super) const MB_INITIAL_RPS: f64 = 4.0;
/// How many consecutive successes before ramping up by 1 RPS.
pub(super) const MB_RAMP_SUCCESS_WINDOW: u32 = 20;

/// Adaptive rate limiter for MusicBrainz.
///
/// Starts at a conservative rate (~4 RPS), ramps up toward the configured
/// ceiling after sustained success, and halves on rate-limit responses.
/// Logs every rate adjustment with the RPS at which failure occurred.
pub(super) struct AdaptiveRateLimiter {
    /// Current interval between requests (1/current_rps).
    current_interval: Duration,
    /// Minimum RPS floor (won't drop below this on backoff).
    min_rps: f64,
    /// Maximum RPS ceiling from config.
    max_rps: f64,
    /// Current effective RPS (tracked as f64 for smooth ramping).
    current_rps: f64,
    /// When the last request was issued.
    last_request_at: Option<Instant>,
    /// Consecutive successes since last failure (for ramp-up gating).
    consecutive_successes: u32,
    /// RPS values at which rate-limit failures occurred (for heuristics).
    pub(super) failure_rps_history: Vec<f64>,
}

impl AdaptiveRateLimiter {
    pub(super) fn new(max_rps: u32) -> Self {
        let max = (max_rps.max(1) as f64).max(MB_INITIAL_RPS);
        let initial = MB_INITIAL_RPS.min(max);
        Self {
            current_interval: Self::interval_for_rps(initial),
            min_rps: 1.0,
            max_rps: max,
            current_rps: initial,
            last_request_at: None,
            consecutive_successes: 0,
            failure_rps_history: Vec::new(),
        }
    }

    /// For local mirrors: start at the ceiling immediately, no ramp-up needed.
    pub(super) fn new_unthrottled(max_rps: u32) -> Self {
        let max = max_rps.max(1) as f64;
        Self {
            current_interval: Self::interval_for_rps(max),
            min_rps: max,
            max_rps: max,
            current_rps: max,
            last_request_at: None,
            consecutive_successes: 0,
            failure_rps_history: Vec::new(),
        }
    }

    fn interval_for_rps(rps: f64) -> Duration {
        Duration::from_micros((1_000_000.0 / rps) as u64)
    }

    /// Duration until this limiter is ready for another request.
    pub(super) fn time_until_ready(&self) -> Duration {
        match self.last_request_at {
            None => Duration::ZERO,
            Some(last) => self.current_interval.saturating_sub(last.elapsed()),
        }
    }

    /// Mark that a request was just dispatched.
    pub(super) fn mark_request(&mut self) {
        self.last_request_at = Some(Instant::now());
    }

    /// Record a successful response. After enough consecutive successes,
    /// ramp up rate by ~1 RPS toward the ceiling.
    pub(super) fn record_success(&mut self) {
        self.consecutive_successes += 1;
        if self.consecutive_successes >= MB_RAMP_SUCCESS_WINDOW && self.current_rps < self.max_rps {
            let old_rps = self.current_rps;
            self.current_rps = (self.current_rps + 1.0).min(self.max_rps);
            self.current_interval = Self::interval_for_rps(self.current_rps);
            self.consecutive_successes = 0;
            crate::logging::log_general(format!(
                "[FETCH] MB rate ramp-up: {:.1} -> {:.1} RPS (after {} clean results)",
                old_rps, self.current_rps, MB_RAMP_SUCCESS_WINDOW,
            ));
        }
    }

    /// Rate-limit hit: halve the current rate, log the failure RPS.
    pub(super) fn apply_backoff(&mut self) {
        let failed_at = self.current_rps;
        self.failure_rps_history.push(failed_at);
        self.consecutive_successes = 0;

        let new_rps = (self.current_rps / 2.0).max(self.min_rps);
        crate::logging::log_general(format!(
            "[FETCH] MB rate backoff: {:.1} -> {:.1} RPS (rate-limited at {:.1}, \
             failure history: {:?})",
            self.current_rps, new_rps, failed_at, self.failure_rps_history,
        ));
        self.current_rps = new_rps;
        self.current_interval = Self::interval_for_rps(self.current_rps);
    }

    /// Clear last-request timestamp (used when a cache hit skips HTTP).
    pub(super) fn clear_last_request(&mut self) {
        self.last_request_at = None;
    }

    /// Current effective RPS (for logging).
    pub(super) fn current_rps(&self) -> f64 {
        self.current_rps
    }
}
